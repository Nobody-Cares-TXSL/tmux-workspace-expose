use std::{
    cell::{Ref, RefCell},
    collections::{HashMap, HashSet},
    thread,
    time::{Duration, Instant},
};

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::{
    appearance::{ACTIVE_REFRESH, Appearance, MAX_CAPTURES, PANE_REFRESH},
    model::Session,
    tmux::capture_pane_preview,
};

#[derive(Clone, Copy)]
pub struct RefreshPolicy {
    pub selected_pane: Duration,
    pub selected_session: Duration,
    pub background: Duration,
    pub max_captures: usize,
}

impl Default for RefreshPolicy {
    fn default() -> Self {
        Self::from_appearance(&Appearance::default())
    }
}

impl RefreshPolicy {
    pub fn from_appearance(appearance: &Appearance) -> Self {
        Self {
            selected_pane: Duration::from_millis(appearance.number(ACTIVE_REFRESH)),
            selected_session: Duration::from_millis(700),
            background: Duration::from_millis(appearance.number(PANE_REFRESH)),
            max_captures: appearance.number(MAX_CAPTURES) as usize,
        }
    }
}

pub struct CachedPreview {
    pub lines: Vec<String>,
    pub failed: bool,
    attempted_at: Instant,
    rendered: [RefCell<Option<RenderedPreview>>; 2],
}

#[derive(Clone, Copy)]
pub(crate) enum PreviewView {
    Thumbnail,
    Screen,
}

struct RenderedPreview {
    width: u16,
    height: u16,
    lines: Vec<Line<'static>>,
}

impl CachedPreview {
    pub(crate) fn rendered(
        &self,
        width: u16,
        height: u16,
        view: PreviewView,
    ) -> Ref<'_, [Line<'static>]> {
        let cache = &self.rendered[match view {
            PreviewView::Thumbnail => 0,
            PreviewView::Screen => 1,
        }];
        if !cache
            .borrow()
            .as_ref()
            .is_some_and(|entry| entry.width == width && entry.height == height)
        {
            let lines = self.preview_rows(width, height, view);
            cache.replace(Some(RenderedPreview {
                width,
                height,
                lines,
            }));
        }
        Ref::map(cache.borrow(), |entry| {
            entry.as_ref().unwrap().lines.as_slice()
        })
    }

    fn preview_rows(&self, width: u16, height: u16, view: PreviewView) -> Vec<Line<'static>> {
        let rows = match view {
            PreviewView::Thumbnail => {
                let limit = if width >= 30 && height >= 6 { 5 } else { 2 };
                let count = usize::from(height.saturating_sub(1).min(limit));
                let end = self
                    .lines
                    .iter()
                    .rposition(|line| !line.trim().is_empty())
                    .map_or(0, |index| index + 1);
                self.lines[end.saturating_sub(count)..end]
                    .iter()
                    .map(String::as_str)
                    .collect()
            }
            PreviewView::Screen => {
                let height = usize::from(height);
                let mut rows = vec![""; height];
                if height != 0 {
                    for (index, line) in self.lines.iter().enumerate() {
                        let row = index * height / self.lines.len();
                        if !line.trim().is_empty() && (rows[row].is_empty() || row >= height / 2) {
                            rows[row] = line;
                        }
                    }
                }
                rows
            }
        };
        rows.into_iter()
            .map(|line| ansi_to_line(line, usize::from(width)))
            .collect()
    }
}

#[derive(Default)]
pub struct PreviewCache {
    by_pane: HashMap<String, CachedPreview>,
    excluded_pane: Option<String>,
}

impl PreviewCache {
    pub fn new(excluded_pane: Option<String>) -> Self {
        Self {
            excluded_pane,
            ..Self::default()
        }
    }

    pub fn get(&self, pane_id: &str) -> Option<&CachedPreview> {
        self.by_pane.get(pane_id)
    }

    pub fn reconcile(&mut self, sessions: &[Session]) {
        let live: HashSet<_> = sessions.iter().flat_map(pane_ids).collect();
        self.by_pane.retain(|id, _| live.contains(id.as_str()));
    }

    fn is_eligible(&self, pane_id: &str) -> bool {
        self.excluded_pane.as_deref() != Some(pane_id)
    }

    pub fn refresh(
        &mut self,
        visible: &[&Session],
        selected_session: Option<&str>,
        selected_pane: Option<&str>,
        policy: RefreshPolicy,
    ) -> bool {
        let due = self.due_panes(
            visible,
            selected_session,
            selected_pane,
            Instant::now(),
            policy,
        );
        thread::scope(|scope| {
            let mut changed = false;
            let jobs: Vec<_> = due
                .into_iter()
                .map(|id| (id, scope.spawn(move || capture_pane_preview(id))))
                .collect();
            for (id, job) in jobs {
                let result = job
                    .join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("capture worker failed")));
                changed |= self.record(id.to_owned(), Instant::now(), result);
            }
            changed
        })
    }

    fn due_panes<'a>(
        &self,
        visible: &[&'a Session],
        selected_session: Option<&str>,
        selected_pane: Option<&str>,
        now: Instant,
        policy: RefreshPolicy,
    ) -> Vec<&'a str> {
        let mut due = HashMap::new();
        for session in visible {
            for id in pane_ids(session).filter(|id| self.is_eligible(id)) {
                let (priority, ttl) = if Some(id) == selected_pane {
                    (0, policy.selected_pane)
                } else if Some(session.id.as_str()) == selected_session {
                    (1, policy.selected_session)
                } else {
                    (2, policy.background)
                };
                let last = self.get(id).map(|preview| preview.attempted_at);
                if last.is_none_or(|last| now.saturating_duration_since(last) >= ttl) {
                    let rank = (priority, last, id);
                    due.entry(id)
                        .and_modify(|old| *old = rank.min(*old))
                        .or_insert(rank);
                }
            }
        }
        let mut ranked: Vec<_> = due.into_values().collect();
        ranked.sort_unstable();
        ranked
            .into_iter()
            .take(policy.max_captures)
            .map(|(_, _, id)| id)
            .collect()
    }

    fn record(&mut self, id: String, now: Instant, result: anyhow::Result<Vec<String>>) -> bool {
        let preview = self.by_pane.entry(id).or_insert_with(|| CachedPreview {
            lines: Vec::new(),
            failed: false,
            attempted_at: now,
            rendered: Default::default(),
        });
        preview.attempted_at = now;
        let failed = result.is_err();
        let changed = preview.failed != failed;
        preview.failed = failed;
        if let Ok(lines) = result
            && lines != preview.lines
        {
            preview.lines = lines;
            preview.rendered = Default::default();
            return true;
        }
        changed
    }
}

fn pane_ids(session: &Session) -> impl Iterator<Item = &str> {
    session
        .windows
        .iter()
        .flat_map(|window| window.panes.iter().map(|pane| pane.id.as_str()))
}

fn ansi_to_line(value: &str, mut remaining: usize) -> Line<'static> {
    let mut spans = Vec::new();
    let mut buffer = String::new();
    let mut style = Style::default();
    let mut chars = value.chars().peekable();

    while remaining > 0
        && let Some(ch) = chars.next()
    {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            let mut sequence = String::new();
            let mut is_sgr = false;
            for next in chars.by_ref() {
                if next == 'm' {
                    is_sgr = true;
                    break;
                }
                if ('@'..='~').contains(&next) {
                    break;
                }
                sequence.push(next);
            }

            if is_sgr {
                flush_span(&mut spans, &mut buffer, style);
                apply_sgr(&sequence, &mut style);
            }
        } else {
            buffer.push(ch);
            remaining -= 1;
        }
    }

    flush_span(&mut spans, &mut buffer, style);
    Line::from(spans)
}

fn flush_span(spans: &mut Vec<Span<'static>>, buffer: &mut String, style: Style) {
    if !buffer.is_empty() {
        spans.push(Span::styled(std::mem::take(buffer), style));
    }
}

fn apply_sgr(sequence: &str, style: &mut Style) {
    let params: Vec<u16> = if sequence.is_empty() {
        vec![0]
    } else {
        sequence
            .split(';')
            .map(|part| part.parse::<u16>().unwrap_or(0))
            .collect()
    };
    let mut index = 0;

    while index < params.len() {
        match params[index] {
            0 => *style = Style::default(),
            1 => *style = style.add_modifier(Modifier::BOLD),
            3 => *style = style.add_modifier(Modifier::ITALIC),
            4 => *style = style.add_modifier(Modifier::UNDERLINED),
            22 => *style = style.remove_modifier(Modifier::BOLD),
            23 => *style = style.remove_modifier(Modifier::ITALIC),
            24 => *style = style.remove_modifier(Modifier::UNDERLINED),
            30..=37 => *style = style.fg(ansi_color(params[index] - 30, false)),
            39 => style.fg = None,
            40..=47 => *style = style.bg(ansi_color(params[index] - 40, false)),
            49 => style.bg = None,
            90..=97 => *style = style.fg(ansi_color(params[index] - 90, true)),
            100..=107 => *style = style.bg(ansi_color(params[index] - 100, true)),
            38 | 48 => {
                let is_fg = params[index] == 38;
                if params.get(index + 1) == Some(&5) {
                    if let Some(color) = params
                        .get(index + 2)
                        .and_then(|value| u8::try_from(*value).ok())
                    {
                        if is_fg {
                            *style = style.fg(Color::Indexed(color));
                        } else {
                            *style = style.bg(Color::Indexed(color));
                        }
                    }
                    index += 2;
                } else if params.get(index + 1) == Some(&2) {
                    let rgb = params.get(index + 2..index + 5).and_then(|values| {
                        values
                            .iter()
                            .map(|value| u8::try_from(*value).ok())
                            .collect::<Option<Vec<_>>>()
                    });
                    if let Some(rgb) = rgb {
                        let color = Color::Rgb(rgb[0], rgb[1], rgb[2]);
                        if is_fg {
                            *style = style.fg(color);
                        } else {
                            *style = style.bg(color);
                        }
                    }
                    index += 4;
                }
            }
            _ => {}
        }
        index += 1;
    }
}

fn ansi_color(index: u16, bright: bool) -> Color {
    match (index, bright) {
        (0, false) => Color::Black,
        (1, false) => Color::Red,
        (2, false) => Color::Green,
        (3, false) => Color::Yellow,
        (4, false) => Color::Blue,
        (5, false) => Color::Magenta,
        (6, false) => Color::Cyan,
        (7, false) => Color::Gray,
        (0, true) => Color::DarkGray,
        (1, true) => Color::LightRed,
        (2, true) => Color::LightGreen,
        (3, true) => Color::LightYellow,
        (4, true) => Color::LightBlue,
        (5, true) => Color::LightMagenta,
        (6, true) => Color::LightCyan,
        (7, true) => Color::White,
        _ => Color::Reset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PanePreview, WindowPreview};
    use crate::tmux::parse_sessions;

    #[test]
    fn ansi_foreground_colors_become_styled_spans() {
        let line = ansi_to_line("plain \u{1b}[31mred\u{1b}[0m done", 80);

        assert_eq!(line.spans.len(), 3);
        assert_eq!(line.spans[0].content, "plain ");
        assert_eq!(line.spans[0].style.fg, None);
        assert_eq!(line.spans[1].content, "red");
        assert_eq!(line.spans[1].style.fg, Some(Color::Red));
        assert_eq!(line.spans[2].content, " done");
        assert_eq!(line.spans[2].style.fg, None);
    }

    #[test]
    fn ansi_truncation_counts_only_visible_characters() {
        let line = ansi_to_line("\u{1b}[31mred\u{1b}[0m plain", 5);
        assert_eq!(
            line.spans,
            [
                Span::styled("red", Style::default().fg(Color::Red)),
                Span::raw(" p"),
            ]
        );
        assert!(ansi_to_line("\u{1b}[31mred", 0).spans.is_empty());
    }

    fn sessions() -> Vec<Session> {
        let mut sessions = parse_sessions("$1:dev:0:1:0:0\n$2:ops:0:1:0:0\n");
        let pane = |id: &str| PanePreview {
            id: id.into(),
            index: 0,
            left: 0,
            top: 0,
            width: 80,
            height: 24,
            active: true,
            command: "sh".into(),
        };
        let window = |panes| WindowPreview {
            id: "@1".into(),
            index: 0,
            name: "shell".into(),
            width: 80,
            height: 24,
            active: true,
            panes,
        };
        sessions[0]
            .windows
            .push(window(vec![pane("%1"), pane("%2")]));
        sessions[1]
            .windows
            .push(window(vec![pane("%2"), pane("%3")]));
        sessions
    }

    #[test]
    fn session_overview_renders_pane_screens_and_split_dividers() {
        use crate::{model::App, ui};
        use ratatui::{Terminal, backend::TestBackend};

        let mut sessions = sessions();
        for session in &mut sessions {
            let panes = &mut session.windows[0].panes;
            panes[0].width = 40;
            panes[1].left = 41;
            panes[1].width = 39;
        }
        sessions[1].windows[0].panes[0].id = "%5".into();
        let panes = &mut sessions[0].windows[0].panes;
        panes[1].height = 11;
        panes.push(PanePreview {
            id: "%4".into(),
            index: 2,
            top: 12,
            height: 12,
            ..panes[1].clone()
        });
        let mut cache = PreviewCache::default();
        for (id, height) in [("%1", 24), ("%2", 11), ("%3", 24), ("%4", 12)] {
            let mut lines = vec![String::new(); height];
            lines[0] = format!("top-{id}");
            lines[height - 1] = format!("bottom-{id}");
            cache.record(id.into(), Instant::now(), Ok(lines));
        }

        let app = App::new(sessions, None);
        let mut terminal = Terminal::new(TestBackend::new(160, 29)).unwrap();
        terminal
            .draw(|frame| ui::render_view(frame, &app, ui::CardColors::default(), None, &cache))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row = |y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        };

        let first = row(2);
        assert!(
            first.contains("top-%1") && first.contains("top-%3"),
            "{first}"
        );
        assert!(
            first.contains("│top-%2") && first.contains("│top-%3"),
            "{first}"
        );
        assert!(row(13).contains("┼─"));
        assert!(row(14).contains("│top-%4"));
        let last = row(25);
        assert!(
            last.contains("bottom-%1") && last.contains("bottom-%3"),
            "{last}"
        );
    }

    #[test]
    fn refreshes_visible_panes_in_priority_order_without_duplicate_captures() {
        let sessions = sessions();
        let visible = [&sessions[1], &sessions[0]];
        let mut cache = PreviewCache::default();
        let policy = RefreshPolicy {
            selected_pane: Duration::from_millis(300),
            selected_session: Duration::from_millis(700),
            background: Duration::from_millis(1500),
            max_captures: 3,
        };
        let now = Instant::now();
        let due = |cache: &PreviewCache, elapsed| {
            cache
                .due_panes(&visible, Some("$1"), Some("%1"), now + elapsed, policy)
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert_eq!(due(&cache, Duration::ZERO), ["%1", "%2", "%3"]);
        for id in ["%1", "%2", "%3"] {
            cache.record(id.into(), now, Ok(vec![id.into()]));
        }
        assert!(due(&cache, Duration::from_millis(299)).is_empty());
        assert_eq!(due(&cache, Duration::from_millis(300)), ["%1"]);
        assert_eq!(due(&cache, Duration::from_millis(700)), ["%1", "%2"]);
        assert_eq!(due(&cache, Duration::from_millis(1500)), ["%1", "%2", "%3"]);
        assert!(
            cache
                .due_panes(&[&sessions[0]], None, None, now + policy.background, policy)
                .iter()
                .all(|id| *id != "%3")
        );
    }

    #[test]
    fn retains_last_success_on_failure_and_discards_deleted_panes() {
        let mut cache = PreviewCache::default();
        let now = Instant::now();
        cache.record("%2".into(), now, Ok(vec!["last output".into()]));
        cache.record("%2".into(), now, Err(anyhow::anyhow!("pane unavailable")));
        cache.record("%9".into(), now, Ok(vec!["deleted".into()]));
        cache.reconcile(&sessions());
        let preview = cache.get("%2").unwrap();
        assert_eq!(preview.lines, ["last output"]);
        assert!(preview.failed);
        assert!(cache.get("%9").is_none());
        cache.record("%2".into(), now, Ok(vec!["recovered".into()]));
        let preview = cache.get("%2").unwrap();
        assert_eq!(preview.lines, ["recovered"]);
        assert!(!preview.failed);
    }

    #[test]
    fn rendered_preview_survives_failed_captures_and_updates_on_content_change() {
        let mut cache = PreviewCache::default();
        let now = Instant::now();
        cache.record("%2".into(), now, Ok(vec!["\u{1b}[31mabcdef".into()]));
        let preview = cache.get("%2").unwrap();
        let rows = preview.rendered(3, 1, PreviewView::Screen);
        assert_eq!(rows[0].spans[0].content, "abc");
        assert_eq!(rows[0].spans[0].style.fg, Some(Color::Red));
        assert_eq!(
            rows.as_ptr(),
            preview.rendered(3, 1, PreviewView::Screen).as_ptr()
        );
        let cached = rows.as_ptr();
        drop(rows);

        cache.record("%2".into(), now, Err(anyhow::anyhow!("pane unavailable")));
        let preview = cache.get("%2").unwrap();
        assert_eq!(cached, preview.rendered(3, 1, PreviewView::Screen).as_ptr());
        assert_eq!(
            preview.rendered(5, 1, PreviewView::Screen)[0].spans[0].content,
            "abcde"
        );

        cache.record("%2".into(), now, Ok(vec!["new".into()]));
        let preview = cache.get("%2").unwrap();
        assert_eq!(
            preview.rendered(5, 1, PreviewView::Screen)[0].spans[0].content,
            "new"
        );
    }

    #[test]
    fn only_changed_preview_content_or_failure_needs_a_redraw() {
        let mut cache = PreviewCache::default();
        let now = Instant::now();
        assert!(cache.record("%2".into(), now, Ok(vec!["output".into()])));
        assert!(!cache.record("%2".into(), now, Ok(vec!["output".into()])));
        assert!(cache.record("%2".into(), now, Err(anyhow::anyhow!("capture failed"))));
        assert!(!cache.record(
            "%2".into(),
            now,
            Err(anyhow::anyhow!("capture failed again"))
        ));
        assert!(cache.record("%2".into(), now, Ok(vec!["output".into()])));
    }

    #[test]
    fn cap_preserves_selection_priority_and_excludes_the_picker_pane() {
        let sessions = sessions();
        let visible = [&sessions[0], &sessions[1]];
        let policy = RefreshPolicy {
            max_captures: 1,
            ..RefreshPolicy::default()
        };
        let now = Instant::now();
        let cache = PreviewCache::default();
        assert_eq!(
            cache.due_panes(&visible, Some("$1"), Some("%2"), now, policy),
            ["%2"]
        );
        let cache = PreviewCache::new(Some("%2".into()));
        assert_eq!(
            cache.due_panes(&visible, Some("$1"), Some("%2"), now, policy),
            ["%1"]
        );
    }
}
