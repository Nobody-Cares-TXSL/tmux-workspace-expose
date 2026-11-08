use crate::appearance::{Appearance, AppearanceEdit, Language, Page, VIM_KEYS};
use crate::grid::{Settings, SettingsEdit};

/// A Claude Code (or other agent) pane's status, as written by an external
/// hook into the tmux pane option `@agent_status`. Ordered least-to-most
/// severe so the derived `Ord` can be used to pick the worst value when
/// folding a session's panes down to one badge: `Attention` (blocked on
/// you) outranks `Waiting` (its turn just ended) outranks `Working` (still running).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AgentStatus {
    Working,
    Waiting,
    Attention,
}

impl AgentStatus {
    pub const ALL: [Self; 3] = [Self::Working, Self::Waiting, Self::Attention];

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "working" => Some(Self::Working),
            "waiting" => Some(Self::Waiting),
            "attention" => Some(Self::Attention),
            _ => None,
        }
    }
}

/// How many panes in a session sit at each agent status. Used to render a
/// count badge (e.g. "2 waiting, 1 working") instead of collapsing a
/// multi-agent session down to a single pane's state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentPaneCounts {
    pub working: u32,
    pub waiting: u32,
    pub attention: u32,
}

impl AgentPaneCounts {
    pub fn count(self, status: AgentStatus) -> u32 {
        match status {
            AgentStatus::Working => self.working,
            AgentStatus::Waiting => self.waiting,
            AgentStatus::Attention => self.attention,
        }
    }

    pub fn total(&self) -> u32 {
        self.working + self.waiting + self.attention
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub attached: bool,
    pub window_count: u32,
    pub windows: Vec<WindowPreview>,
    /// The worst-ranked status among this session's panes, or `None` if no
    /// pane has ever reported one.
    pub agent_status: Option<AgentStatus>,
    /// Unix timestamp (seconds) of the oldest pane at `agent_status`'s rank
    /// — i.e. how long the most-neglected pane has held that status.
    pub agent_status_since: Option<i64>,
    pub agent_pane_counts: AgentPaneCounts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowPreview {
    pub id: String,
    pub index: u32,
    pub name: String,
    pub width: u16,
    pub height: u16,
    pub active: bool,
    pub panes: Vec<PanePreview>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanePreview {
    pub id: String,
    pub index: u32,
    pub left: u16,
    pub top: u16,
    pub width: u16,
    pub height: u16,
    pub active: bool,
    pub command: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    Overview,
    Session,
}

#[derive(Debug)]
pub enum WorkspaceTarget {
    Session { id: String, name: String },
    Window { id: String, name: String },
}

impl WorkspaceTarget {
    pub fn description(&self) -> String {
        self.description_in(Language::En)
    }

    pub fn description_in(&self, lang: Language) -> String {
        match self {
            Self::Session { name, .. } => format!("{} {name}", lang.text("会话", "Session")),
            Self::Window { name, .. } => format!("{} {name}", lang.text("窗口", "Window")),
        }
    }
}

#[derive(Debug)]
pub enum WorkspaceDialog {
    Close(WorkspaceTarget),
    Rename(WorkspaceTarget, String),
}

#[derive(Debug)]
pub struct App {
    pub sessions: Vec<Session>,
    pub selected_index: usize,
    pub current_session_name: Option<String>,
    pub should_quit: bool,
    pub should_switch: bool,
    pub error: Option<String>,
    pub focus: Focus,
    pub dialog: Option<WorkspaceDialog>,
    pub confirmed_action: Option<WorkspaceDialog>,
    pub grid: Settings,
    pub grid_edit: Option<SettingsEdit>,
    pub appearance: Appearance,
    pub appearance_edit: Option<AppearanceEdit>,
    pub settings_page: Page,
    pub save_grid: bool,
    pub grid_error: Option<String>,
    pub session_row: usize,
    pub window_row: usize,
    window_selection: Option<String>,
    search_query: Option<String>,
    /// True while actively typing a query. In vim mode this is distinct from
    /// `search_query` being set: Esc leaves editing (so hjkl moves again) but
    /// keeps `search_query` as an applied filter, matching how Telescope-style
    /// pickers commit a search into normal-mode browsing instead of discarding
    /// it. Default mode has no normal-mode navigation to return to, so the two
    /// always stay in lockstep there — see `is_searching`.
    editing_search: bool,
}

impl App {
    pub fn new(sessions: Vec<Session>, current_session_name: Option<String>) -> Self {
        let selected_index = current_session_name
            .as_ref()
            .and_then(|name| sessions.iter().position(|session| &session.name == name))
            .unwrap_or(0);

        Self {
            sessions,
            selected_index,
            current_session_name,
            should_quit: false,
            should_switch: false,
            error: None,
            focus: Focus::Overview,
            dialog: None,
            confirmed_action: None,
            grid: Settings::default(),
            grid_edit: None,
            appearance: Appearance::default(),
            appearance_edit: None,
            settings_page: Page::Grid,
            save_grid: false,
            grid_error: None,
            session_row: 0,
            window_row: 0,
            window_selection: None,
            search_query: None,
            editing_search: false,
        }
    }

    /// Moves the cursor to the top card when an agent there is blocked on or
    /// waiting for you. Meant to run after
    /// `sort_sessions_by_agent_status_with_current`, so the top card is the
    /// most urgent one other than the current session
    /// (which sorts last within its rank); otherwise the cursor stays on the
    /// current session. A merely `Working` agent doesn't need you yet, so it
    /// doesn't pull the cursor away.
    pub fn select_most_urgent_agent_session(&mut self) {
        let needs_you = self.sessions.first().is_some_and(|session| {
            matches!(
                session.agent_status,
                Some(AgentStatus::Attention | AgentStatus::Waiting)
            )
        });
        if needs_you {
            self.selected_index = 0;
        }
    }

    pub fn selected_session(&self) -> Option<&Session> {
        self.visible_session_iter().nth(self.selected_index)
    }

    pub fn selected_window(&self) -> Option<&WindowPreview> {
        let windows = &self.selected_session()?.windows;
        self.window_selection
            .as_ref()
            .and_then(|id| windows.iter().find(|window| &window.id == id))
            .or_else(|| windows.iter().find(|window| window.active))
            .or_else(|| windows.first())
    }

    pub fn select_window(&mut self, window_id: String) {
        self.window_selection = Some(window_id);
    }

    pub fn selected_window_index(&self) -> usize {
        let Some(selected) = self.selected_window() else {
            return 0;
        };
        self.selected_session()
            .and_then(|session| {
                session
                    .windows
                    .iter()
                    .position(|window| window.id == selected.id)
            })
            .unwrap_or(0)
    }

    pub fn move_window_row(&mut self, columns: usize, down: bool) {
        let Some(session) = self.selected_session() else {
            return;
        };
        let index = self.selected_window_index();
        let row = index / columns;
        let next = if down && row + 1 < session.windows.len().div_ceil(columns) {
            (index + columns).min(session.windows.len() - 1)
        } else if !down && row > 0 {
            index - columns
        } else {
            return;
        };
        self.select_window(session.windows[next].id.clone());
    }

    pub fn cycle_window(&mut self, backward: bool) {
        let Some(session) = self
            .selected_session()
            .filter(|session| !session.windows.is_empty())
        else {
            return;
        };
        let windows = &session.windows;
        let index = self.selected_window_index();
        let step = if backward { windows.len() - 1 } else { 1 };
        self.select_window(windows[(index + step) % windows.len()].id.clone());
    }

    pub fn open_session_detail(&mut self) {
        if self.selected_session().is_some() {
            self.stop_editing_search();
            self.window_selection = None;
            self.window_row = 0;
            self.focus = Focus::Session;
        }
    }

    pub fn open_current_windows(&mut self) {
        let Some(index) = self
            .sessions
            .iter()
            .position(|session| self.current_session_name.as_deref() == Some(&session.name))
        else {
            return;
        };
        self.clear_search();
        self.selected_index = index;
        self.open_session_detail();
    }

    pub fn open_grid_settings(&mut self) {
        self.grid_edit = Some(SettingsEdit::new(self.grid));
        self.appearance_edit = Some(AppearanceEdit::new(self.appearance.clone()));
        self.settings_page = Page::Grid;
        self.grid_error = None;
    }

    pub fn close_settings(&mut self) {
        self.grid_edit = None;
        self.appearance_edit = None;
        self.grid_error = None;
    }

    pub fn select_settings_page(&mut self, page: Page) {
        self.settings_page = page;
        if page != Page::Grid
            && let Some(edit) = &mut self.appearance_edit
        {
            edit.select(page.fields().start);
        }
    }

    pub(crate) fn settings_field(&self) -> usize {
        let first = self.settings_page.fields().start;
        match self.settings_page {
            Page::Grid => self.grid_edit.as_ref().map_or(first, |edit| edit.field),
            _ => self
                .appearance_edit
                .as_ref()
                .map_or(first, |edit| edit.field),
        }
    }

    pub fn workspace_target(&self) -> Option<WorkspaceTarget> {
        let session = self.selected_session()?;
        match self.focus {
            Focus::Overview => Some(WorkspaceTarget::Session {
                id: session.id.clone(),
                name: session.name.clone(),
            }),
            Focus::Session => self
                .selected_window()
                .map(|window| WorkspaceTarget::Window {
                    id: window.id.clone(),
                    name: format!("{}:{} ({})", session.name, window.index, window.name),
                }),
        }
    }

    pub fn visible_sessions(&self) -> Vec<&Session> {
        self.visible_session_iter().collect()
    }

    fn visible_session_iter(&self) -> impl Iterator<Item = &Session> {
        let query = self
            .search_query
            .as_deref()
            .filter(|query| !query.is_empty())
            .map(str::to_lowercase);
        self.sessions.iter().filter(move |session| {
            query
                .as_deref()
                .is_none_or(|query| fuzzy_matches(&session.name, query))
        })
    }

    pub fn visible_session_count(&self) -> usize {
        if self.search_query.as_deref().is_none_or(str::is_empty) {
            self.sessions.len()
        } else {
            self.visible_session_iter().count()
        }
    }

    pub fn start_search(&mut self) {
        self.search_query = Some(String::new());
        self.editing_search = true;
        self.selected_index = 0;
    }

    pub fn push_search_char(&mut self, ch: char) {
        if let Some(query) = &mut self.search_query {
            query.push(ch);
            self.selected_index = 0;
        }
    }

    pub fn pop_search_char(&mut self) {
        if let Some(query) = &mut self.search_query {
            query.pop();
            self.selected_index = 0;
        }
    }

    /// Deletes the word before the end of the query, like readline's Ctrl-W:
    /// trailing whitespace first, then the word itself.
    pub fn pop_search_word(&mut self) {
        if let Some(query) = &mut self.search_query {
            let kept = query
                .trim_end()
                .trim_end_matches(|ch: char| !ch.is_whitespace());
            query.truncate(kept.len());
            self.selected_index = 0;
        }
    }

    /// Empties the query but stays in search, like readline's Ctrl-U.
    pub fn clear_search_text(&mut self) {
        if let Some(query) = &mut self.search_query {
            query.clear();
            self.selected_index = 0;
        }
    }

    pub fn clear_search(&mut self) {
        self.search_query = None;
        self.editing_search = false;
        self.selected_index = 0;
    }

    /// Leaves text-entry but keeps `search_query` as an applied filter — vim
    /// mode's Esc-while-searching, so hjkl navigates the filtered results
    /// instead of discarding them.
    pub fn stop_editing_search(&mut self) {
        self.editing_search = false;
    }

    /// Whether keystrokes should be treated as search text-entry right now.
    /// Also drives the toggle-key/typeable-filter check in `input.rs`. In
    /// vim mode this is `false` while a filter is applied but not being
    /// edited; in default mode it always matches `search_query.is_some()`,
    /// since default mode has no normal-mode navigation to drop into.
    pub fn is_searching(&self) -> bool {
        self.editing_search
    }

    pub fn popup_state(&self) -> String {
        let session = self
            .selected_session()
            .map_or("-", |session| session.id.as_str());
        let window = self
            .selected_window()
            .map_or("-", |window| window.id.as_str());
        let edit = self
            .grid_edit
            .as_ref()
            .map_or_else(String::new, SettingsEdit::encode);
        let page = self.settings_page.index();
        let appearance_edit = self
            .appearance_edit
            .as_ref()
            .map_or_else(String::new, AppearanceEdit::snapshot);
        format!(
            "{session} {window} {} {} {} {} {}\n{edit}\n{page} {appearance_edit}\n{}",
            u8::from(self.focus == Focus::Session),
            self.session_row,
            self.window_row,
            u8::from(self.search_query.is_some()),
            u8::from(self.editing_search),
            self.search_text().unwrap_or_default()
        )
    }

    pub fn restore_popup_state(&mut self, saved: &str) {
        let Some((meta, rest)) = saved.split_once('\n') else {
            return;
        };
        let Some((edit, rest)) = rest.split_once('\n') else {
            return;
        };
        let Some((appearance_edit, query)) = rest.split_once('\n') else {
            return;
        };
        let fields: Vec<_> = meta.split_whitespace().collect();
        let [
            session,
            window,
            detail,
            session_row,
            window_row,
            has_query,
            editing,
        ] = fields.as_slice()
        else {
            return;
        };
        self.search_query = (*has_query == "1").then(|| query.to_owned());
        self.editing_search = self.search_query.is_some() && *editing == "1";
        let index = self
            .visible_session_iter()
            .position(|entry| entry.id == *session);
        self.selected_index = index.unwrap_or_else(|| {
            self.selected_index
                .min(self.visible_session_count().saturating_sub(1))
        });
        if index.is_some() && *detail == "1" {
            self.focus = Focus::Session;
            self.select_window((*window).to_owned());
        }
        self.session_row = session_row.parse().unwrap_or(0);
        self.window_row = window_row.parse().unwrap_or(0);
        self.grid_edit = SettingsEdit::decode(edit);
        if self.grid_edit.is_some()
            && let Some((page, snapshot)) = appearance_edit.split_once(' ')
        {
            self.settings_page = match page {
                "1" => Page::Popup,
                "2" => Page::Colors,
                "3" => Page::Behavior,
                _ => Page::Grid,
            };
            self.appearance_edit = AppearanceEdit::restore(snapshot)
                .or_else(|| Some(AppearanceEdit::new(self.appearance.clone())));
        }
    }

    pub fn search_text(&self) -> Option<&str> {
        self.search_query.as_deref()
    }

    pub fn vim_keys(&self) -> bool {
        self.appearance.0[VIM_KEYS] == "on"
    }

    pub fn language(&self) -> Language {
        self.appearance.language()
    }

    pub fn replace_sessions(&mut self, sessions: Vec<Session>) {
        let selected_id = self.selected_session().map(|session| session.id.clone());
        self.sessions = sessions;

        if self.visible_session_count() == 0 {
            self.selected_index = 0;
            self.focus = Focus::Overview;
            return;
        }

        self.selected_index = selected_id
            .as_ref()
            .and_then(|id| {
                self.visible_session_iter()
                    .position(|session| &session.id == id)
            })
            .unwrap_or_else(|| self.selected_index.min(self.visible_session_count() - 1));
        if self.selected_session().map(|session| &session.id) != selected_id.as_ref() {
            self.focus = Focus::Overview;
        }
    }

    pub fn move_left(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    pub fn move_right(&mut self) {
        if self.selected_index + 1 < self.visible_session_count() {
            self.selected_index += 1;
        }
    }

    pub fn move_up(&mut self, columns: usize) {
        let columns = columns.max(1);
        if self.selected_index >= columns {
            self.selected_index -= columns;
        }
    }

    pub fn move_down(&mut self, columns: usize) {
        let columns = columns.max(1);
        let visible_count = self.visible_session_count();
        if visible_count == 0 {
            return;
        }

        let last_index = visible_count - 1;
        let current_row = self.selected_index / columns;
        let last_row = last_index / columns;
        if current_row < last_row {
            self.selected_index = self.selected_index.saturating_add(columns).min(last_index);
        }
    }
}

/// Reorders sessions so the ones an agent is waiting on you for lead the
/// grid: `Attention` first, then `Waiting`, then `Working`, then sessions
/// with no agent at all. Within a rank, the longest-waiting session (oldest
/// `agent_status_since`) sorts first, so a neglected pane doesn't get
/// buried under one that only just finished. The current session sorts last
/// within its rank: you're already looking at it, so the other sessions at
/// that urgency are the ones worth surfacing. Stable, so untracked sessions
/// keep tmux's own ordering relative to each other.
pub fn sort_sessions_by_agent_status_with_current(
    sessions: &mut [Session],
    current_session_name: Option<&str>,
) {
    sessions.sort_by_key(|session| {
        let is_tracked_current =
            session.agent_status.is_some() && current_session_name == Some(session.name.as_str());
        (
            std::cmp::Reverse(session.agent_status),
            is_tracked_current,
            session.agent_status_since.unwrap_or(i64::MAX),
        )
    });
}

/// Same ordering as [`sort_sessions_by_agent_status_with_current`], without a
/// current-session tiebreak. Kept as its own public function (rather than
/// changing this signature to take an `Option<&str>`) so existing callers of
/// this library function keep compiling.
pub fn sort_sessions_by_agent_status(sessions: &mut [Session]) {
    sort_sessions_by_agent_status_with_current(sessions, None);
}

fn fuzzy_matches(name: &str, query: &str) -> bool {
    let name = name.to_lowercase();
    let mut name_chars = name.chars();
    query
        .chars()
        .all(|query_ch| name_chars.any(|name_ch| name_ch == query_ch))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(name: &str) -> Session {
        Session {
            id: format!("${name}"),
            name: name.to_string(),
            attached: false,
            window_count: 1,
            windows: Vec::new(),
            agent_status: None,
            agent_status_since: None,
            agent_pane_counts: AgentPaneCounts::default(),
        }
    }

    fn session_with_agent_status(
        name: &str,
        status: Option<AgentStatus>,
        since: Option<i64>,
    ) -> Session {
        Session {
            agent_status: status,
            agent_status_since: since,
            ..session(name)
        }
    }

    #[test]
    fn agent_sort_puts_attention_before_waiting_before_working_before_none() {
        let mut sessions = vec![
            session_with_agent_status("idle", None, None),
            session_with_agent_status("working", Some(AgentStatus::Working), Some(1)),
            session_with_agent_status("attention", Some(AgentStatus::Attention), Some(1)),
            session_with_agent_status("waiting", Some(AgentStatus::Waiting), Some(1)),
        ];

        sort_sessions_by_agent_status(&mut sessions);

        let names: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["attention", "waiting", "working", "idle"]);
    }

    #[test]
    fn urgent_agent_session_takes_cursor_from_current_session() {
        let mut sessions = vec![
            session_with_agent_status("current", None, None),
            session_with_agent_status("blocked", Some(AgentStatus::Attention), Some(1)),
        ];
        sort_sessions_by_agent_status(&mut sessions);
        let mut app = App::new(sessions, Some("current".to_string()));
        assert_eq!(app.selected_session().unwrap().name, "current");

        app.select_most_urgent_agent_session();

        assert_eq!(app.selected_session().unwrap().name, "blocked");
    }

    #[test]
    fn working_agent_does_not_take_cursor_from_current_session() {
        let mut sessions = vec![
            session_with_agent_status("current", None, None),
            session_with_agent_status("busy", Some(AgentStatus::Working), Some(1)),
        ];
        sort_sessions_by_agent_status(&mut sessions);
        let mut app = App::new(sessions, Some("current".to_string()));

        app.select_most_urgent_agent_session();

        assert_eq!(app.selected_session().unwrap().name, "current");
    }

    #[test]
    fn agent_sort_puts_current_session_last_within_its_rank() {
        let mut sessions = vec![
            session_with_agent_status("current", Some(AgentStatus::Waiting), Some(1)),
            session_with_agent_status("older", Some(AgentStatus::Waiting), Some(50)),
            session_with_agent_status("newer", Some(AgentStatus::Waiting), Some(100)),
            session_with_agent_status("busy", Some(AgentStatus::Working), Some(1)),
        ];

        sort_sessions_by_agent_status_with_current(&mut sessions, Some("current"));

        let names: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["older", "newer", "current", "busy"]);
    }

    #[test]
    fn agent_sort_keeps_untracked_current_session_in_tmux_order() {
        let mut sessions = vec![
            session_with_agent_status("current", None, None),
            session_with_agent_status("other", None, None),
        ];

        sort_sessions_by_agent_status_with_current(&mut sessions, Some("current"));

        let names: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["current", "other"]);
    }

    #[test]
    fn agent_sort_breaks_ties_by_oldest_timestamp_first() {
        let mut sessions = vec![
            session_with_agent_status("just-now", Some(AgentStatus::Waiting), Some(500)),
            session_with_agent_status("neglected", Some(AgentStatus::Waiting), Some(100)),
        ];

        sort_sessions_by_agent_status(&mut sessions);

        let names: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["neglected", "just-now"]);
    }

    #[test]
    fn agent_sort_is_stable_for_sessions_with_no_agent() {
        let mut sessions = vec![
            session_with_agent_status("zeta", None, None),
            session_with_agent_status("alpha", None, None),
        ];

        sort_sessions_by_agent_status(&mut sessions);

        let names: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["zeta", "alpha"]);
    }

    #[test]
    fn selects_current_session_when_present() {
        let app = App::new(
            vec![session("dev"), session("logs"), session("notes")],
            Some("logs".to_string()),
        );

        assert_eq!(app.selected_index, 1);
    }

    #[test]
    fn clamps_navigation_at_grid_edges() {
        let mut app = App::new(vec![session("one"), session("two"), session("three")], None);

        app.move_left();
        assert_eq!(app.selected_index, 0);

        app.move_right();
        app.move_right();
        app.move_right();
        assert_eq!(app.selected_index, 2);

        app.move_down(2);
        assert_eq!(app.selected_index, 2);

        app.move_up(2);
        assert_eq!(app.selected_index, 0);
    }

    #[test]
    fn preserves_selected_session_by_name_after_refresh() {
        let mut app = App::new(
            vec![session("dev"), session("logs"), session("notes")],
            None,
        );
        app.selected_index = 1;

        app.replace_sessions(vec![session("new"), session("logs"), session("dev")]);

        assert_eq!(app.selected_session().unwrap().name, "logs");
    }

    #[test]
    fn search_filters_sessions_by_fuzzy_name() {
        let mut app = App::new(
            vec![
                session("backend-api"),
                session("frontend"),
                session("database"),
            ],
            None,
        );

        app.start_search();
        app.push_search_char('b');
        app.push_search_char('a');

        let names: Vec<&str> = app
            .visible_sessions()
            .into_iter()
            .map(|session| session.name.as_str())
            .collect();
        assert_eq!(names, vec!["backend-api", "database"]);
    }

    #[test]
    fn selected_session_uses_filtered_selection() {
        let mut app = App::new(
            vec![session("backend"), session("frontend"), session("database")],
            None,
        );

        app.start_search();
        app.push_search_char('f');

        assert_eq!(app.selected_index, 0);
        assert_eq!(app.selected_session().unwrap().name, "frontend");
    }

    #[test]
    fn clearing_search_restores_all_sessions() {
        let mut app = App::new(vec![session("backend"), session("frontend")], None);

        app.start_search();
        app.push_search_char('f');
        app.clear_search();

        assert!(!app.is_searching());
        assert_eq!(app.visible_session_count(), 2);
    }

    #[test]
    fn up_from_first_row_keeps_selection_in_place() {
        let mut app = App::new(vec![session("one"), session("two"), session("three")], None);
        app.selected_index = 1;

        app.move_up(2);

        assert_eq!(app.selected_index, 1);
    }

    #[test]
    fn down_to_incomplete_row_selects_nearest_card() {
        let mut app = App::new(
            vec![
                session("one"),
                session("two"),
                session("three"),
                session("four"),
                session("five"),
            ],
            None,
        );
        app.selected_index = 2;

        app.move_down(3);

        assert_eq!(app.selected_index, 4);
    }
}
