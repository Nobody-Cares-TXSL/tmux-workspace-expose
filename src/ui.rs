use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

use crate::{
    appearance::{FIELDS, LANGUAGE, Language, POPUP_BACKGROUND, POPUP_BORDER, Page, parse_color},
    grid::{FIELD_NAMES, Limits, Settings, Viewport},
    model::{AgentStatus, App, Focus, PanePreview, Session, WindowPreview, WorkspaceDialog},
    preview::PreviewCache,
};

pub const MIN_CARD_WIDTH: u16 = 32;
pub const MIN_CARD_HEIGHT: u16 = 10;
const FOOTER_HEIGHT: u16 = 1;

/// Colors used to highlight session cards by state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardColors {
    /// The currently selected card (title + border).
    pub selected: Color,
    /// The session you are currently attached to (title + border).
    pub attached: Color,
    /// Every other card (title only; the border stays dimmed).
    pub inactive: Color,
    /// A session with a pane blocked on you (e.g. a permission prompt).
    pub attention: Color,
    /// A session where an agent's turn just ended — it's idle on you.
    pub waiting: Color,
    /// A session with an agent still actively running.
    pub working: Color,
}

impl Default for CardColors {
    fn default() -> Self {
        Self {
            selected: Color::Yellow,
            attached: Color::Green,
            inactive: Color::White,
            attention: Color::Rgb(255, 85, 85),
            waiting: Color::Rgb(255, 184, 108),
            working: Color::Rgb(139, 233, 253),
        }
    }
}

fn agent_status_color(status: AgentStatus, colors: CardColors) -> Color {
    match status {
        AgentStatus::Attention => colors.attention,
        AgentStatus::Waiting => colors.waiting,
        AgentStatus::Working => colors.working,
    }
}

fn agent_status_icon(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Attention => "‼",
        AgentStatus::Waiting => "⏳",
        AgentStatus::Working => "⚙",
    }
}

fn agent_status_word(status: AgentStatus, lang: Language) -> &'static str {
    match status {
        AgentStatus::Attention => lang.text("需要处理", "needs you"),
        AgentStatus::Waiting => lang.text("等待回复", "awaiting reply"),
        AgentStatus::Working => lang.text("运行中", "working"),
    }
}

fn agent_status_count(status: AgentStatus, counts: crate::model::AgentPaneCounts) -> u32 {
    match status {
        AgentStatus::Attention => counts.attention,
        AgentStatus::Waiting => counts.waiting,
        AgentStatus::Working => counts.working,
    }
}

/// The bottom-border badge text for a session's agent status. A glyph alone
/// (or a bare count like "⏳2") is a gamble — it depends on the terminal
/// font rendering that character, and tells you nothing if it doesn't. So
/// the status that actually matters — whichever one is worst, since that's
/// what set the card's color and sort position — always gets spelled out
/// in words. Other statuses present on other panes stay compact (e.g.
/// "‼ needs you · ⏳2 · ⚙1") since they're supporting context, not the
/// thing you need to read at a glance.
fn agent_status_label(session: &Session, lang: Language) -> Option<String> {
    let counts = session.agent_pane_counts;
    let worst = session.agent_status?;
    if counts.total() == 0 {
        return None;
    }

    let worst_count = agent_status_count(worst, counts);
    let worst_label = if worst_count > 1 {
        format!(
            "{}{} {}",
            agent_status_icon(worst),
            worst_count,
            agent_status_word(worst, lang)
        )
    } else {
        format!(
            "{} {}",
            agent_status_icon(worst),
            agent_status_word(worst, lang)
        )
    };

    if counts.total() == 1 {
        return Some(worst_label);
    }

    let mut parts = vec![worst_label];
    for status in [
        AgentStatus::Attention,
        AgentStatus::Waiting,
        AgentStatus::Working,
    ] {
        if status == worst {
            continue;
        }
        let count = agent_status_count(status, counts);
        if count > 0 {
            parts.push(format!("{}{}", agent_status_icon(status), count));
        }
    }
    Some(parts.join(" · "))
}

fn agent_status_span(
    session: &Session,
    colors: CardColors,
    lang: Language,
) -> Option<Span<'static>> {
    let label = agent_status_label(session, lang)?;
    let color = agent_status_color(session.agent_status?, colors);
    Some(Span::styled(
        format!(" {label} "),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ))
}

pub fn render_view(
    frame: &mut Frame<'_>,
    app: &App,
    colors: CardColors,
    forced_columns: Option<usize>,
    previews: &PreviewCache,
) {
    let area = frame.area();
    let lang = app.language();
    frame.render_widget(Clear, area);

    if area.width < 20 || area.height < 6 {
        render_centered_message(frame, area, lang.text("终端空间不足", "Terminal too small"));
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(FOOTER_HEIGHT)])
        .split(area);

    if let Some(error) = &app.error
        && app.sessions.is_empty()
    {
        render_centered_message(frame, chunks[0], error);
    } else if app.sessions.is_empty() {
        render_centered_message(
            frame,
            chunks[0],
            lang.text(
                "没有 tmux 会话。\n按 q 退出。",
                "No tmux sessions found.\nPress q to quit.",
            ),
        );
    } else if app.visible_session_count() == 0 {
        render_centered_message(
            frame,
            chunks[0],
            lang.text("没有匹配的会话", "No matching sessions"),
        );
    } else {
        render_grid_view(frame, app, colors, chunks[0], forced_columns, previews);
    }

    let mut footer = workspace_footer(app, chunks[0], forced_columns);
    if !app.sessions.is_empty()
        && let Some(error) = &app.error
    {
        footer.spans.insert(
            0,
            Span::styled(format!("{error} │ "), Style::default().fg(Color::Red)),
        );
    }
    frame.render_widget(Paragraph::new(footer), chunks[1]);
    if let Some(dialog) = &app.dialog {
        render_workspace_dialog(frame, area, dialog, lang);
    }
    if app.grid_edit.is_some() {
        render_settings(frame, app, area, colors.selected);
    }
}

fn render_settings(frame: &mut Frame<'_>, app: &App, area: Rect, selected_color: Color) {
    let Some(grid) = &app.grid_edit else { return };
    let lang = app.language();
    let page = app.settings_page;
    let selected = if page == Page::Grid {
        grid.field
    } else {
        app.appearance_edit
            .as_ref()
            .map_or(page.fields().start, |edit| edit.field)
    };
    let (panel, fields) = settings_panel(area, selected, page.fields());
    frame.render_widget(Clear, panel);
    let mut tab_spans = Vec::new();
    for (name, tab) in settings_tabs(lang) {
        let style = if page == tab {
            Style::default()
                .fg(selected_color)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(Color::Cyan)
        };
        tab_spans.push(Span::styled(name, style));
        tab_spans.push(Span::raw("│"));
    }
    let mut lines = vec![Line::from(tab_spans)];
    for index in fields {
        let cursor = if selected == index { '>' } else { ' ' };
        let line = if page == Page::Grid {
            let chinese = [
                "Session 列数",
                "Session 行数",
                "Window 列数",
                "Window 行数",
                "卡片间距",
            ];
            let label = lang.text(chinese[index], FIELD_NAMES[index]);
            Line::raw(settings_value(cursor, label, &grid.values[index], 18))
        } else if let Some(edit) = &app.appearance_edit {
            let value = &edit.draft.0[index];
            let field = FIELDS[index];
            let label = lang.text(field.name, field.english);
            let shown = if index == LANGUAGE {
                match value.as_str() {
                    "zh" => "zh · 简体中文",
                    "en" => "en · English",
                    _ => value,
                }
            } else {
                value
            };
            let mut spans = vec![Span::raw(settings_value(cursor, label, shown, 31))];
            if (page == Page::Colors || matches!(index, POPUP_BACKGROUND | POPUP_BORDER))
                && let Ok(color) = parse_color(value)
            {
                spans.push(Span::styled("  ■", Style::default().fg(color)));
            }
            Line::from(spans)
        } else {
            continue;
        };
        lines.push(if selected == index {
            line.style(
                Style::default()
                    .fg(selected_color)
                    .add_modifier(Modifier::REVERSED),
            )
        } else {
            line
        });
    }
    if let Some(error) = &app.grid_error {
        lines.push(Line::styled(error, Style::default().fg(Color::Red)));
    } else if page == Page::Popup {
        lines.push(Line::raw(lang.text(
            "宽高支持百分比或字符格；弹窗外观在下次打开时生效。",
            "Width and height use percent or cells. Popup changes apply on reopen.",
        )));
    } else if page == Page::Colors {
        lines.push(Line::raw(lang.text(
            "支持颜色名、colour0–255 和 #RRGGBB。",
            "Use color names, colour0–255 or #RRGGBB.",
        )));
    } else if page == Page::Behavior {
        lines.push(Line::raw(lang.text(
            "语言可用 ←/→ 切换；刷新时间以毫秒计。",
            "Change language with ←/→. Refresh intervals are in milliseconds.",
        )));
    }
    let hint = shortcut_line(
        String::new(),
        &[
            (
                lang.text("切页", "Tab"),
                lang.text("Tab / 点击", "Tab / click"),
            ),
            (lang.text("选项", "Field"), "↑/↓"),
            (
                lang.text("调整", "Edit"),
                lang.text("输入或 ←/→", "type or ←/→"),
            ),
            (lang.text("保存", "Save"), "Enter"),
            (lang.text("取消", "Cancel"), "Esc"),
        ],
    );
    let block = Block::bordered()
        .title(lang.text(" 设置 ", " Settings "))
        .title_bottom(hint);
    frame.render_widget(Paragraph::new(lines).block(block), panel);
}

fn settings_value(cursor: char, label: &str, value: &str, width: usize) -> String {
    let padding = width.saturating_sub(Span::raw(label).width());
    format!("{cursor} {label}{:padding$} [{value}]", "")
}

pub fn settings_tabs(lang: Language) -> [(&'static str, Page); 4] {
    [
        (lang.text(" 网格 ", " Grid "), Page::Grid),
        (lang.text(" 弹窗 ", " Popup "), Page::Popup),
        (lang.text(" 配色 ", " Colors "), Page::Colors),
        (lang.text(" 行为 ", " Behavior "), Page::Behavior),
    ]
}

pub fn settings_tab_at(lang: Language, mut column: usize) -> Option<Page> {
    for (label, page) in settings_tabs(lang) {
        let width = Span::raw(label).width();
        if column < width {
            return Some(page);
        }
        column = column.checked_sub(width + 1)?;
    }
    None
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[test]
    fn clickable_tabs_follow_the_width_of_the_translated_labels() {
        for lang in [Language::Zh, Language::En] {
            let mut x = 0;
            for (label, page) in settings_tabs(lang) {
                let width = Span::raw(label).width();
                assert_eq!(settings_tab_at(lang, x), Some(page));
                assert_eq!(settings_tab_at(lang, x + width - 1), Some(page));
                assert_eq!(settings_tab_at(lang, x + width), None);
                x += width + 1;
            }
        }
    }
}

pub fn settings_panel(
    area: Rect,
    selected: usize,
    fields: std::ops::Range<usize>,
) -> (Rect, std::ops::Range<usize>) {
    let width = area.width.min(88);
    let height = area.height.min((fields.len() + 4) as u16);
    let panel = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let rows = usize::from(height.saturating_sub(4)).min(fields.len());
    let start = (selected + 1)
        .saturating_sub(rows)
        .max(fields.start)
        .min(fields.end - rows);
    (panel, start..start + rows)
}

fn render_workspace_dialog(
    frame: &mut Frame<'_>,
    area: Rect,
    action: &WorkspaceDialog,
    lang: Language,
) {
    let width = area.width.min(70);
    let height = area.height.min(8);
    let dialog = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, dialog);
    let (title, hint, message) = match action {
        WorkspaceDialog::Close(target) => (
            lang.text(" 确认关闭 ", " Confirm close "),
            lang.text(" Enter 关闭 · Esc/n 取消 ", " Enter Close · Esc/n Cancel "),
            format!(
                "{} {}?",
                lang.text("关闭", "Close"),
                target.description_in(lang)
            ),
        ),
        WorkspaceDialog::Rename(target, name) => (
            lang.text(" 重命名 ", " Rename "),
            lang.text(" Enter 保存 · Esc 取消 ", " Enter Save · Esc Cancel "),
            format!(
                "{}\n{} {name}▏",
                target.description_in(lang),
                lang.text("新名称：", "New name:")
            ),
        ),
    };
    let block = Block::bordered().title(title).title_bottom(hint);
    frame.render_widget(
        Paragraph::new(message)
            .block(block)
            .wrap(Wrap { trim: false }),
        dialog,
    );
}

fn workspace_footer(app: &App, area: Rect, columns: Option<usize>) -> Line<'static> {
    let lang = app.language();
    let count = if app.focus == Focus::Session {
        app.selected_session()
            .map_or(0, |session| session.windows.len())
    } else {
        app.visible_session_count()
    };
    let visible = workspace_viewport(app, area, columns).range();
    let range = format!(
        "{}–{}/{count}",
        if visible.is_empty() {
            0
        } else {
            visible.start + 1
        },
        visible.end
    );
    if app.focus == Focus::Session {
        return shortcut_line(
            range,
            &[
                (lang.text("退出", "Quit"), "q"),
                (
                    lang.text("选择", "Select"),
                    if app.vim_keys() {
                        lang.text("hjkl / Tab / 滚轮", "hjkl / Tab / wheel")
                    } else {
                        lang.text("方向键 / Tab / 滚轮", "arrows / Tab / wheel")
                    },
                ),
                (lang.text("切换", "Switch"), "Enter"),
                (lang.text("重命名", "Rename"), "r"),
                (lang.text("关闭", "Close"), "d"),
                (lang.text("设置", "Settings"), "Ctrl+,"),
                (lang.text("返回", "Back"), "Esc"),
            ],
        );
    }
    if app.is_searching() {
        return shortcut_line(
            format!(
                "{}{}",
                lang.text("搜索：", "Search: "),
                app.search_text().unwrap_or_default()
            ),
            &[
                (lang.text("打开", "Open"), "Enter"),
                (
                    if app.vim_keys() {
                        lang.text("完成编辑", "Done")
                    } else {
                        lang.text("清除", "Clear")
                    },
                    "Esc",
                ),
            ],
        );
    }
    let lead = app.search_text().map_or_else(
        || range.clone(),
        |query| format!("{range} {}{query}", lang.text("筛选：", "Filter: ")),
    );
    shortcut_line(
        lead,
        &[
            (lang.text("退出", "Quit"), "q"),
            (
                lang.text("选择", "Select"),
                if app.vim_keys() {
                    lang.text("hjkl / 滚轮", "hjkl / wheel")
                } else {
                    lang.text("方向键 / 滚轮", "arrows / wheel")
                },
            ),
            (lang.text("打开", "Open"), "Enter"),
            (lang.text("搜索", "Search"), "/"),
            (lang.text("重命名", "Rename"), "r"),
            (lang.text("关闭", "Close"), "d"),
            (lang.text("当前 Session", "Current session"), "Alt+w"),
            (lang.text("设置", "Settings"), "Ctrl+,"),
        ],
    )
}

fn shortcut_line(lead: String, shortcuts: &[(&str, &str)]) -> Line<'static> {
    let label_style = Style::default().fg(Color::Rgb(110, 158, 181));
    let key_style = Style::default()
        .fg(Color::Rgb(151, 211, 231))
        .add_modifier(Modifier::BOLD);
    let mut spans = Vec::new();
    if !lead.is_empty() {
        spans.push(Span::styled(lead, label_style));
    }
    for (label, key) in shortcuts {
        if !spans.is_empty() {
            spans.push(Span::styled(" │ ", label_style));
        }
        spans.push(Span::styled(format!("{label}: "), label_style));
        spans.push(Span::styled(format!("<{key}>"), key_style));
    }
    Line::from(spans)
}

fn render_grid_view(
    frame: &mut Frame<'_>,
    app: &App,
    colors: CardColors,
    area: Rect,
    forced_columns: Option<usize>,
    previews: &PreviewCache,
) {
    let sessions = app.visible_sessions();
    for (index, card_area) in session_cards(app, area, forced_columns) {
        if let Some(session) = sessions.get(index) {
            let current_attached = session.attached
                && app.current_session_name.as_deref() == Some(session.name.as_str());
            let selected = index == app.selected_index;
            let lang = app.language();
            let mut block =
                card_block(session, selected, current_attached, colors, card_area, lang);
            if app.focus == Focus::Overview {
                let count = session.windows.len();
                let visible = window_viewport(card_area, app.grid.windows, count, 0, 0)
                    .range()
                    .end;
                if visible < count && card_area.width >= 26 {
                    block = block.title_bottom(
                        Line::from(format!(
                            " {visible}/{count} {} ",
                            lang.text("窗口", "windows")
                        ))
                        .right_aligned(),
                    );
                }
            }
            frame.render_widget(block, card_area);
            let window = if selected && app.focus == Focus::Session {
                app.selected_window()
            } else {
                None
            };
            let first = if app.focus == Focus::Session {
                app.window_row
            } else {
                0
            };
            let tiles = workspace_layout(
                session,
                card_area,
                window.map(|window| window.id.as_str()),
                app.grid,
                first,
            );
            render_workspace_card(frame, tiles, previews, window, colors.selected, lang);
        }
    }
}

pub fn session_cards(app: &App, area: Rect, forced_columns: Option<usize>) -> Vec<(usize, Rect)> {
    if app.focus == Focus::Session && app.selected_index < app.visible_session_count() {
        return vec![(app.selected_index, area)];
    }
    workspace_viewport(app, area, forced_columns)
        .cards(area, app.grid.gap)
        .collect()
}

pub fn workspace_viewport(app: &App, area: Rect, columns: Option<usize>) -> Viewport {
    if app.focus == Focus::Session {
        let count = app
            .selected_session()
            .map_or(0, |session| session.windows.len());
        return window_viewport(
            area,
            app.grid.windows,
            count,
            app.selected_window_index(),
            app.window_row,
        );
    }
    let limits = Limits {
        columns: columns.unwrap_or(app.grid.sessions.columns),
        rows: app.grid.sessions.rows,
    };
    Viewport::new(
        area,
        limits,
        (MIN_CARD_WIDTH, MIN_CARD_HEIGHT),
        app.visible_session_count(),
        app.selected_index,
        app.session_row,
    )
}

fn window_viewport(
    card: Rect,
    limits: Limits,
    count: usize,
    selected: usize,
    first: usize,
) -> Viewport {
    Viewport::new(
        card.inner(Margin::new(1, 1)),
        limits,
        (18, 5),
        count,
        selected,
        first,
    )
}

fn card_block(
    session: &Session,
    selected: bool,
    current_attached: bool,
    colors: CardColors,
    area: Rect,
    lang: Language,
) -> Block<'static> {
    let title = format!(
        " {} ",
        truncate(&session.name, area.width.saturating_sub(12) as usize)
    );
    let mut bottom_spans = vec![session_status_span(session.attached, lang)];
    if let Some(badge) = agent_status_span(session, colors, lang) {
        bottom_spans.push(badge);
    }
    Block::default()
        .title(Span::styled(
            title,
            card_title_style(selected, current_attached, colors),
        ))
        .title_bottom(Line::from(bottom_spans))
        .borders(Borders::ALL)
        .border_type(if selected {
            BorderType::Double
        } else {
            BorderType::Plain
        })
        .border_style(card_border_style(
            selected,
            current_attached,
            session.agent_status,
            colors,
        ))
}

struct WindowTile<'a> {
    window: &'a WindowPreview,
    area: Rect,
    screen: bool,
    panes: Vec<(&'a PanePreview, Rect)>,
}

fn workspace_layout<'a>(
    session: &'a Session,
    card: Rect,
    selected: Option<&str>,
    settings: Settings,
    first: usize,
) -> Vec<WindowTile<'a>> {
    let inner = card.inner(Margin::new(1, 1));
    let windows = &session.windows;
    if inner.is_empty() || windows.is_empty() {
        return Vec::new();
    }
    let selected_index = windows
        .iter()
        .position(|window| selected == Some(window.id.as_str()))
        .unwrap_or(0);
    let full_preview = selected.is_some();
    window_viewport(card, settings.windows, windows.len(), selected_index, first)
        .cards(inner, settings.gap)
        .map(|(index, area)| {
            let window = &windows[index];
            let content = Block::default().borders(window_borders(area)).inner(area);
            let (panes, compact) = pane_layout(window, content, full_preview);
            WindowTile {
                window,
                area,
                screen: full_preview && !compact,
                panes,
            }
        })
        .collect()
}

fn window_borders(area: Rect) -> Borders {
    if area.width >= 3 && area.height >= 3 {
        Borders::ALL
    } else if area.height > 1 {
        Borders::TOP
    } else {
        Borders::NONE
    }
}

fn grid_cell(area: Rect, index: usize, columns: usize, rows: usize) -> Rect {
    let col = index % columns;
    let row = index / columns;
    let x = |i| (i * usize::from(area.width) / columns) as u16;
    let y = |i| (i * usize::from(area.height) / rows) as u16;
    Rect::new(
        area.x + x(col),
        area.y + y(row),
        x(col + 1) - x(col),
        y(row + 1) - y(row),
    )
}

fn pane_layout(
    window: &WindowPreview,
    area: Rect,
    split: bool,
) -> (Vec<(&PanePreview, Rect)>, bool) {
    let mut scaled = Vec::new();
    for pane in &window.panes {
        let rect = scale_pane(pane, window, area, split);
        if rect.is_empty()
            || scaled
                .iter()
                .any(|(_, prior): &(_, Rect)| prior.intersects(rect))
        {
            return (compact_panes(window, area), true);
        }
        scaled.push((pane, rect));
    }
    (scaled, false)
}

fn scale_pane(pane: &PanePreview, window: &WindowPreview, area: Rect, split: bool) -> Rect {
    let x = |edge: u16| scale_edge(edge, window.width, area.width);
    let y = |edge: u16| scale_edge(edge, window.height, area.height);
    let mut left = x(pane.left);
    let mut top = y(pane.top);
    if split && pane.left > 0 {
        left = left.max(x(pane.left - 1) + 1);
    }
    if split && pane.top > 0 {
        top = top.max(y(pane.top - 1) + 1);
    }
    let right = x(pane.left.saturating_add(pane.width));
    let bottom = y(pane.top.saturating_add(pane.height));
    Rect::new(
        area.x + left,
        area.y + top,
        right.saturating_sub(left),
        bottom.saturating_sub(top),
    )
}

fn scale_edge(edge: u16, source: u16, target: u16) -> u16 {
    (u32::from(edge.min(source)) * u32::from(target) / u32::from(source.max(1))) as u16
}

fn compact_panes(window: &WindowPreview, area: Rect) -> Vec<(&PanePreview, Rect)> {
    let count = window
        .panes
        .len()
        .min(usize::from(area.width) * usize::from(area.height));
    let rows = count.min(usize::from(area.height));
    if rows == 0 {
        return Vec::new();
    }
    let columns = count.div_ceil(rows);
    window
        .panes
        .iter()
        .take(count)
        .enumerate()
        .map(|(index, pane)| (pane, grid_cell(area, index, columns, rows)))
        .collect()
}

pub fn window_at(app: &App, card: Rect, x: u16, y: u16) -> Option<String> {
    let session = app.selected_session()?;
    let selected = app.selected_window().map(|window| window.id.as_str());
    workspace_layout(session, card, selected, app.grid, app.window_row)
        .into_iter()
        .find(|tile| tile.area.contains((x, y).into()))
        .map(|tile| tile.window.id.clone())
}

fn render_workspace_card(
    frame: &mut Frame<'_>,
    tiles: Vec<WindowTile<'_>>,
    cache: &PreviewCache,
    selected: Option<&WindowPreview>,
    selection_color: Color,
    lang: Language,
) {
    for tile in tiles {
        let window = tile.window;
        let title = format!(
            "{} {} · {} {}",
            window.index,
            window.name,
            window.panes.len(),
            lang.text(
                "窗格",
                if window.panes.len() == 1 {
                    "pane"
                } else {
                    "panes"
                }
            )
        );
        let is_selected = selected.is_some_and(|target| target.id == window.id);
        let border_color = if is_selected {
            selection_color
        } else if window.active {
            Color::Cyan
        } else {
            Color::Gray
        };
        let block = Block::default()
            .borders(window_borders(tile.area))
            .border_type(if is_selected {
                BorderType::Double
            } else {
                BorderType::Plain
            })
            .border_style(Style::default().fg(border_color))
            .title(format!(" {title} "));
        for &(pane, area) in &tile.panes {
            if tile.screen {
                render_pane_screen(frame, pane, cache, area, lang);
            } else if tile.area.height > 1 {
                render_pane(frame, pane, cache, area);
            }
        }
        frame.render_widget(block, tile.area);
        if tile.screen {
            render_split_dividers(frame, &tile.panes);
        }
    }
}

fn render_pane(frame: &mut Frame<'_>, pane: &PanePreview, cache: &PreviewCache, area: Rect) {
    let preview = cache.get(&pane.id);
    let color = match preview {
        Some(preview) if preview.failed => Color::Red,
        _ if pane.active => Color::Cyan,
        _ => Color::Gray,
    };
    let label = format!("{} {}", pane.id, pane.command);
    let mut lines = vec![Line::styled(label, Style::default().fg(color))];
    if area.width >= 15
        && area.height >= 3
        && let Some(preview) = preview
    {
        let max_lines = if area.width >= 30 && area.height >= 6 {
            5
        } else {
            2
        };
        let count = usize::from((area.height - 1).min(max_lines));
        let end = preview
            .lines
            .iter()
            .rposition(|line| !line.trim().is_empty())
            .map_or(0, |row| row + 1);
        lines.extend(
            preview.lines[end.saturating_sub(count)..end]
                .iter()
                .map(|line| ansi_to_line(&truncate_ansi(line, usize::from(area.width)))),
        );
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_pane_screen(
    frame: &mut Frame<'_>,
    pane: &PanePreview,
    cache: &PreviewCache,
    area: Rect,
    lang: Language,
) {
    let Some(preview) = cache.get(&pane.id) else {
        return;
    };
    if preview.failed && preview.lines.is_empty() {
        frame.render_widget(
            Paragraph::new(lang.text("预览暂不可用", "Preview unavailable"))
                .style(Style::default().fg(Color::Red)),
            area,
        );
        return;
    }
    let mut rows = vec![""; usize::from(area.height)];
    let source_height = preview.lines.len().max(1);
    for (index, line) in preview.lines.iter().enumerate() {
        let row = index * rows.len() / source_height;
        if !line.trim().is_empty() && (rows[row].is_empty() || row >= rows.len() / 2) {
            rows[row] = line;
        }
    }
    let lines: Vec<_> = rows
        .into_iter()
        .map(|line| ansi_to_line(&truncate_ansi(line, usize::from(area.width))))
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_split_dividers(frame: &mut Frame<'_>, panes: &[(&PanePreview, Rect)]) {
    for &(pane, area) in panes {
        let color = if pane.active {
            Color::Green
        } else {
            Color::Gray
        };
        if pane.left > 0 {
            for y in area.top() - u16::from(pane.top > 0)..area.bottom() {
                render_divider_cell(frame, area.left() - 1, y, "│", color);
            }
        }
        if pane.top > 0 {
            for x in area.left() - u16::from(pane.left > 0)..area.right() {
                render_divider_cell(frame, x, area.top() - 1, "─", color);
            }
        }
    }
}

fn render_divider_cell(frame: &mut Frame<'_>, x: u16, y: u16, glyph: &str, color: Color) {
    if let Some(cell) = frame.buffer_mut().cell_mut((x, y)) {
        let glyph = match (cell.symbol(), glyph) {
            ("│", "─") | ("─", "│") | ("┼", _) => "┼",
            _ => glyph,
        };
        cell.set_symbol(glyph).set_fg(color);
    }
}

fn card_title_style(selected: bool, current_attached: bool, colors: CardColors) -> Style {
    if selected {
        Style::default()
            .fg(colors.selected)
            .add_modifier(Modifier::BOLD)
    } else if current_attached {
        Style::default()
            .fg(colors.attached)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(colors.inactive)
            .add_modifier(Modifier::BOLD)
    }
}

fn card_border_style(
    selected: bool,
    current_attached: bool,
    agent_status: Option<AgentStatus>,
    colors: CardColors,
) -> Style {
    if selected {
        Style::default()
            .fg(colors.selected)
            .add_modifier(Modifier::BOLD)
    } else if current_attached {
        Style::default().fg(colors.attached)
    } else if let Some(status) = agent_status {
        Style::default().fg(agent_status_color(status, colors))
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

fn session_status_span(attached: bool, lang: Language) -> Span<'static> {
    if attached {
        Span::styled(
            lang.text(" 已连接 ", " attached "),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            lang.text(" 未连接 ", " detached "),
            Style::default().fg(Color::DarkGray),
        )
    }
}

fn render_centered_message(frame: &mut Frame<'_>, area: Rect, message: &str) {
    let paragraph = Paragraph::new(message)
        .style(Style::default().fg(Color::Gray))
        .alignment(ratatui::layout::Alignment::Center)
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

fn truncate(value: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }

    let mut output = String::new();
    for ch in value.chars().take(max_width) {
        output.push(ch);
    }
    output
}

fn truncate_ansi(value: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }

    let mut output = String::new();
    let mut visible_width = 0;
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            output.push(ch);
            output.push(chars.next().unwrap());
            for next in chars.by_ref() {
                output.push(next);
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
            continue;
        }

        if visible_width == max_width {
            break;
        }

        output.push(ch);
        visible_width += 1;
    }

    output
}

fn ansi_to_line(value: &str) -> Line<'static> {
    let mut spans = Vec::new();
    let mut buffer = String::new();
    let mut style = Style::default();
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.next() {
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
    use ratatui::layout::Rect;

    use super::*;

    #[test]
    fn attached_status_is_highlighted() {
        let span = session_status_span(true, Language::En);

        assert_eq!(span.content, " attached ");
        assert_eq!(span.style.fg, Some(Color::Green));
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn detached_status_stays_muted() {
        let span = session_status_span(false, Language::En);

        assert_eq!(span.content, " detached ");
        assert_eq!(span.style.fg, Some(Color::DarkGray));
        assert!(!span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn selected_session_title_is_yellow_and_bold() {
        let style = card_title_style(true, false, CardColors::default());

        assert_eq!(style.fg, Some(Color::Yellow));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn current_attached_session_title_is_green_and_bold() {
        let style = card_title_style(false, true, CardColors::default());

        assert_eq!(style.fg, Some(Color::Green));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn other_session_titles_are_white_and_bold() {
        let style = card_title_style(false, false, CardColors::default());

        assert_eq!(style.fg, Some(Color::White));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn current_attached_session_border_is_green_when_not_selected() {
        let style = card_border_style(false, true, None, CardColors::default());

        assert_eq!(style.fg, Some(Color::Green));
        assert!(!style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn selected_session_border_stays_yellow_and_bold() {
        let style = card_border_style(true, true, None, CardColors::default());

        assert_eq!(style.fg, Some(Color::Yellow));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn custom_colors_override_card_styles() {
        let colors = CardColors {
            selected: Color::Magenta,
            attached: Color::Blue,
            inactive: Color::Cyan,
            attention: Color::Red,
            waiting: Color::LightYellow,
            working: Color::LightCyan,
        };

        assert_eq!(
            card_title_style(true, false, colors).fg,
            Some(Color::Magenta)
        );
        assert_eq!(
            card_border_style(true, false, None, colors).fg,
            Some(Color::Magenta)
        );
        assert_eq!(card_title_style(false, true, colors).fg, Some(Color::Blue));
        assert_eq!(
            card_border_style(false, true, None, colors).fg,
            Some(Color::Blue)
        );
        assert_eq!(card_title_style(false, false, colors).fg, Some(Color::Cyan));
        // Unselected, unattached, no-agent border stays dimmed regardless of
        // the configured color.
        assert_eq!(
            card_border_style(false, false, None, colors).fg,
            Some(Color::DarkGray)
        );
    }

    #[test]
    fn agent_status_colors_take_priority_over_dimmed_default_border() {
        let colors = CardColors::default();

        assert_eq!(
            card_border_style(false, false, Some(AgentStatus::Attention), colors).fg,
            Some(colors.attention)
        );
        assert_eq!(
            card_border_style(false, false, Some(AgentStatus::Waiting), colors).fg,
            Some(colors.waiting)
        );
        assert_eq!(
            card_border_style(false, false, Some(AgentStatus::Working), colors).fg,
            Some(colors.working)
        );
    }

    #[test]
    fn selection_and_attachment_still_outrank_agent_status_color() {
        let colors = CardColors::default();

        assert_eq!(
            card_border_style(true, false, Some(AgentStatus::Attention), colors).fg,
            Some(colors.selected)
        );
        assert_eq!(
            card_border_style(false, true, Some(AgentStatus::Attention), colors).fg,
            Some(colors.attached)
        );
    }

    #[test]
    fn single_pane_agent_status_renders_a_plain_label() {
        let mut session = test_session("dev");
        session.agent_status = Some(AgentStatus::Waiting);
        session.agent_pane_counts = crate::model::AgentPaneCounts {
            waiting: 1,
            ..Default::default()
        };

        assert_eq!(
            agent_status_label(&session, Language::En),
            Some("⏳ awaiting reply".to_string())
        );
    }

    #[test]
    fn multi_pane_agent_status_spells_out_the_worst_status_and_counts_the_rest() {
        let mut session = test_session("dev");
        session.agent_status = Some(AgentStatus::Attention);
        session.agent_pane_counts = crate::model::AgentPaneCounts {
            working: 1,
            waiting: 2,
            attention: 1,
        };

        assert_eq!(
            agent_status_label(&session, Language::En),
            Some("‼ needs you · ⏳2 · ⚙1".to_string())
        );
    }

    #[test]
    fn multi_pane_agent_status_shows_a_count_on_the_worst_status_too_when_it_has_more_than_one() {
        let mut session = test_session("dev");
        session.agent_status = Some(AgentStatus::Attention);
        session.agent_pane_counts = crate::model::AgentPaneCounts {
            working: 0,
            waiting: 1,
            attention: 2,
        };

        assert_eq!(
            agent_status_label(&session, Language::En),
            Some("‼2 needs you · ⏳1".to_string())
        );
    }

    #[test]
    fn no_agent_pane_status_renders_no_label() {
        let session = test_session("dev");

        assert_eq!(agent_status_label(&session, Language::En), None);
    }

    fn test_session(name: &str) -> Session {
        Session {
            id: format!("${name}"),
            name: name.to_string(),
            attached: false,
            window_count: 1,
            windows: Vec::new(),
            agent_status: None,
            agent_status_since: None,
            agent_pane_counts: crate::model::AgentPaneCounts::default(),
        }
    }

    #[test]
    fn ansi_foreground_colors_become_styled_spans() {
        let line = ansi_to_line("plain \u{1b}[31mred\u{1b}[0m done");

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
        let truncated = truncate_ansi("\u{1b}[31mred\u{1b}[0m plain", 5);

        assert_eq!(truncated, "\u{1b}[31mred\u{1b}[0m p");
    }
    #[test]
    fn workspace_layout_scales_splits_inside_each_window() {
        let mut session = test_session("work");
        let pane = |id: &str, index, left, width| crate::model::PanePreview {
            id: id.into(),
            index,
            left,
            top: 0,
            width,
            height: 24,
            active: index == 0,
            command: "sh".into(),
        };
        let window = |index, panes| crate::model::WindowPreview {
            id: format!("@{index}"),
            index,
            name: format!("window {index}"),
            width: 80,
            height: 24,
            active: index == 0,
            panes,
        };
        session.windows = vec![
            window(0, vec![pane("%1", 0, 0, 40), pane("%2", 1, 41, 39)]),
            window(1, vec![pane("%3", 0, 0, 80)]),
        ];
        let area = Rect::new(10, 4, 42, 14);
        let settings = Settings::default();
        let layout = workspace_layout(&session, area, None, settings, 0);
        assert_eq!(layout.len(), 2);
        assert_eq!(layout[0].panes[0].1, Rect::new(12, 6, 8, 10));
        assert_eq!(layout[0].panes[1].1, Rect::new(20, 6, 9, 10));
        assert_eq!(layout[1].panes[0].1, Rect::new(32, 6, 18, 10));

        let area = Rect::new(0, 0, 100, 24);
        let detail = workspace_layout(&session, area, Some("@0"), settings, 0);
        assert!(detail[0].screen);
        assert!(detail[1].screen);
        let next = workspace_layout(&session, area, Some("@1"), settings, 0);
        assert_eq!(detail[0].panes, next[0].panes);
        assert_eq!(detail[0].screen, next[0].screen);
        assert_eq!(detail[0].area.right() + 1, detail[1].area.left());
        assert!(detail[0].panes[0].1.right() < detail[0].panes[1].1.left());
    }
}
