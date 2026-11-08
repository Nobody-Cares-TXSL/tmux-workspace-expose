use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use crate::{
    appearance::Page,
    grid::FIELD_NAMES,
    model::{App, Focus, WorkspaceDialog},
    ui,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToggleKey {
    code: KeyCode,
    modifiers: KeyModifiers,
}

impl ToggleKey {
    pub fn from_tmux_key(value: &str) -> Option<Self> {
        let (mut modifiers, key) = if let Some(key) = value.strip_prefix("M-") {
            (KeyModifiers::ALT, key)
        } else if let Some(key) = value.strip_prefix("C-") {
            (KeyModifiers::CONTROL, key)
        } else {
            (KeyModifiers::NONE, value)
        };

        let code = match key {
            "Esc" => KeyCode::Esc,
            key if key.chars().count() == 1 => {
                let ch = key.chars().next()?;
                if ch.is_ascii_uppercase() {
                    modifiers.insert(KeyModifiers::SHIFT);
                }
                KeyCode::Char(ch)
            }
            _ => return None,
        };

        Some(Self { code, modifiers })
    }

    fn matches(self, key: KeyEvent) -> bool {
        self.code == key.code && self.modifiers == key.modifiers
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent, columns: usize) {
    handle_key_with_toggle(app, key, columns, None);
}

pub fn handle_key_with_toggle(
    app: &mut App,
    key: KeyEvent,
    columns: usize,
    toggle_key: Option<ToggleKey>,
) {
    if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
        app.should_quit = true;
        return;
    }

    if toggle_key.is_some_and(|toggle_key| toggle_key.matches(key)) {
        // A typeable toggle key — e.g. `e`/`E` from a `prefix + e` binding — must
        // stay usable as filter input while searching, so only close on it when a
        // query is not being entered. A modified toggle (e.g. M-e) can't be typed
        // into the filter, so it still closes from anywhere.
        let editing = app.is_searching() || matches!(app.dialog, Some(WorkspaceDialog::Rename(..)));
        if !(editing && is_typeable_filter_key(key)) {
            app.should_quit = true;
            return;
        }
    }

    if app.dialog.is_some() {
        handle_dialog_key(app, key);
        return;
    }

    if app.grid_edit.is_some() {
        handle_grid_key(app, key);
        return;
    }
    if key.kind == KeyEventKind::Press && settings_key(key) {
        app.open_grid_settings();
        return;
    }
    if key.kind == KeyEventKind::Press
        && key.code == KeyCode::Char('w')
        && key.modifiers == KeyModifiers::ALT
    {
        app.open_current_windows();
        return;
    }

    if handle_workspace_key(app, key, columns) {
        return;
    }

    if app.is_searching() {
        handle_search_key(app, key, columns);
        return;
    }

    handle_navigation_key(app, key, columns);
}

fn settings_key(key: KeyEvent) -> bool {
    matches!(
        (key.code, key.modifiers),
        (KeyCode::Char(','), KeyModifiers::CONTROL) | (KeyCode::F(12), KeyModifiers::SHIFT)
    )
}

fn handle_grid_key(app: &mut App, key: KeyEvent) {
    if key.kind == KeyEventKind::Press && (settings_key(key) || key.code == KeyCode::Esc) {
        app.close_settings();
        return;
    }
    if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) && key.kind == KeyEventKind::Press {
        app.select_settings_page(
            app.settings_page
                .step(key.code == KeyCode::BackTab || key.modifiers.contains(KeyModifiers::SHIFT)),
        );
        return;
    }
    if key.code == KeyCode::Enter && key.kind == KeyEventKind::Press {
        app.save_grid = true;
        return;
    }
    app.grid_error = None;
    if app.settings_page != Page::Grid {
        let Some(edit) = &mut app.appearance_edit else {
            return;
        };
        let fields = app.settings_page.fields();
        match key.code {
            KeyCode::Up => edit.select(if edit.field == fields.start {
                fields.end - 1
            } else {
                edit.field - 1
            }),
            KeyCode::Down => edit.select(if edit.field + 1 == fields.end {
                fields.start
            } else {
                edit.field + 1
            }),
            KeyCode::Left | KeyCode::Right => edit.adjust(key.code == KeyCode::Right),
            KeyCode::Char(ch) if ch.is_ascii_graphic() && is_typeable_filter_key(key) => {
                edit.insert(ch)
            }
            KeyCode::Backspace => edit.backspace(),
            _ => {}
        }
        return;
    }
    let Some(draft) = &mut app.grid_edit else {
        return;
    };
    match key.code {
        KeyCode::Up => draft.select((draft.field + FIELD_NAMES.len() - 1) % FIELD_NAMES.len()),
        KeyCode::Down => draft.select((draft.field + 1) % FIELD_NAMES.len()),
        KeyCode::Left | KeyCode::Right => draft.adjust(key.code == KeyCode::Right),
        KeyCode::Char(ch) if ch.is_ascii_digit() && key.modifiers == KeyModifiers::NONE => {
            draft.digit(ch)
        }
        KeyCode::Backspace => draft.backspace(),
        _ => {}
    }
}

fn handle_dialog_key(app: &mut App, key: KeyEvent) {
    match (key.code, key.modifiers, key.kind) {
        (KeyCode::Esc, _, KeyEventKind::Press) => app.dialog = None,
        (KeyCode::Enter, _, KeyEventKind::Press) => {
            if !matches!(&app.dialog, Some(WorkspaceDialog::Rename(_, name)) if name.trim().is_empty())
            {
                app.confirmed_action = app.dialog.take();
            }
        }
        (KeyCode::Char('n'), KeyModifiers::NONE, _)
            if matches!(app.dialog, Some(WorkspaceDialog::Close(_))) =>
        {
            app.dialog = None;
        }
        _ => {
            if let Some(WorkspaceDialog::Rename(_, name)) = &mut app.dialog {
                match (key.code, key.modifiers) {
                    (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT)
                        if !ch.is_control() =>
                    {
                        name.push(ch)
                    }
                    (KeyCode::Backspace, _) => {
                        name.pop();
                    }
                    (KeyCode::Char('u'), KeyModifiers::CONTROL) => name.clear(),
                    _ => {}
                }
            }
        }
    }
}

fn handle_workspace_key(app: &mut App, key: KeyEvent, columns: usize) -> bool {
    if key.kind == KeyEventKind::Repeat && matches!(key.code, KeyCode::Enter | KeyCode::Esc) {
        return true;
    }
    if let KeyCode::Char(ch @ ('d' | 'r')) = key.code
        && key.modifiers == KeyModifiers::NONE
        && !app.is_searching()
    {
        if key.kind == KeyEventKind::Press
            && let Some(target) = app.workspace_target()
        {
            app.dialog = Some(match ch {
                'd' => WorkspaceDialog::Close(target),
                _ => WorkspaceDialog::Rename(target, String::new()),
            });
        }
        return true;
    }
    if key.code == KeyCode::Char('q') && key.modifiers == KeyModifiers::NONE && !app.is_searching()
    {
        app.should_quit = true;
        return true;
    }
    if app.focus == Focus::Session {
        handle_detail_key(app, key, columns.max(1));
        return true;
    }
    match (key.code, key.modifiers) {
        (KeyCode::Enter, _) => app.open_session_detail(),
        (KeyCode::Esc, _) if !app.is_searching() && app.search_text().is_some() => {
            app.clear_search()
        }
        (KeyCode::Char('/'), KeyModifiers::NONE) if !app.is_searching() => app.start_search(),
        _ => return false,
    }
    true
}

fn handle_detail_key(app: &mut App, key: KeyEvent, columns: usize) {
    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) => app.focus = Focus::Overview,
        (KeyCode::Enter, _) => app.should_switch = app.selected_window().is_some(),
        (KeyCode::Down | KeyCode::Up, KeyModifiers::NONE) => {
            app.move_window_row(columns, key.code == KeyCode::Down)
        }
        (KeyCode::Tab | KeyCode::Right, KeyModifiers::NONE) => app.cycle_window(false),
        (KeyCode::BackTab, _)
        | (KeyCode::Tab, KeyModifiers::SHIFT)
        | (KeyCode::Left, KeyModifiers::NONE) => app.cycle_window(true),
        (KeyCode::Char('l' | 'j'), KeyModifiers::NONE) if app.vim_keys => app.cycle_window(false),
        (KeyCode::Char('h' | 'k'), KeyModifiers::NONE) if app.vim_keys => app.cycle_window(true),
        _ => {}
    }
}

fn handle_navigation_key(app: &mut App, key: KeyEvent, columns: usize) {
    let code = match (key.code, key.modifiers) {
        (KeyCode::Char('h'), KeyModifiers::NONE) if app.vim_keys => KeyCode::Left,
        (KeyCode::Char('j'), KeyModifiers::NONE) if app.vim_keys => KeyCode::Down,
        (KeyCode::Char('k'), KeyModifiers::NONE) if app.vim_keys => KeyCode::Up,
        (KeyCode::Char('l'), KeyModifiers::NONE) if app.vim_keys => KeyCode::Right,
        (code, _) => code,
    };
    match code {
        KeyCode::Left => move_left(app, columns),
        KeyCode::Right => move_right(app, columns),
        KeyCode::Up => app.move_up(columns),
        KeyCode::Down => app.move_down(columns),
        _ => {}
    }
}

pub fn handle_mouse(
    app: &mut App,
    mouse: MouseEvent,
    grid_area: Rect,
    forced_columns: Option<usize>,
) {
    if let Some(draft) = &app.grid_edit {
        let area = Rect {
            height: grid_area.height.saturating_add(1),
            ..grid_area
        };
        let fields = app.settings_page.fields();
        let selected = if app.settings_page == Page::Grid {
            draft.field
        } else {
            app.appearance_edit
                .as_ref()
                .map_or(fields.start, |edit| edit.field)
        };
        let (panel, rows) = ui::settings_panel(area, selected, fields);
        let inner = panel.inner(ratatui::layout::Margin::new(1, 1));
        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && contains(inner, mouse.column, mouse.row)
        {
            let row = usize::from(mouse.row - inner.y);
            if row == 0 {
                if let Some(page) =
                    ui::settings_tab_at(app.language(), usize::from(mouse.column - inner.x))
                {
                    app.select_settings_page(page);
                }
            } else if rows.contains(&(rows.start + row - 1)) {
                let index = rows.start + row - 1;
                if app.settings_page == Page::Grid {
                    app.grid_edit.as_mut().unwrap().select(index);
                } else if let Some(edit) = &mut app.appearance_edit {
                    edit.select(index);
                }
                app.grid_error = None;
            }
        }
        return;
    }
    if app.dialog.is_some() {
        return;
    }
    if matches!(
        mouse.kind,
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
    ) {
        let columns = ui::workspace_viewport(app, grid_area, forced_columns).columns;
        let down = mouse.kind == MouseEventKind::ScrollDown;
        if app.focus == Focus::Session {
            app.move_window_row(columns, down);
        } else if down {
            app.move_down(columns);
        } else {
            app.move_up(columns);
        }
        return;
    }
    if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
        return;
    }

    let cards = ui::session_cards(app, grid_area, forced_columns);
    if let Some((index, card)) = cards
        .into_iter()
        .find(|(_, card)| contains(*card, mouse.column, mouse.row))
    {
        app.selected_index = index;
        if app.focus == Focus::Session
            && let Some(id) = ui::window_at(app, card, mouse.column, mouse.row)
        {
            app.select_window(id);
        }
    }
}

fn contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x
        && x < area.x.saturating_add(area.width)
        && y >= area.y
        && y < area.y.saturating_add(area.height)
}

/// Keys the search filter accepts as text input — matching the `Char` arm in
/// `handle_search_key`. Uppercase keys arrive with `SHIFT`, so both lower- and
/// uppercase configured toggle keys count as typeable.
fn is_typeable_filter_key(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(_))
        && matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
}

fn handle_search_key(app: &mut App, key: KeyEvent, columns: usize) {
    match (key.code, key.modifiers) {
        // Vim mode: Esc on an empty query cancels (nothing to keep); Esc on
        // a typed query commits it as a filter and drops into NORMAL mode —
        // hjkl then browses the filtered results instead of typing into it.
        (KeyCode::Esc, _) if app.vim_keys && app.search_text().is_some_and(str::is_empty) => {
            app.clear_search();
        }
        (KeyCode::Esc, _) if app.vim_keys => app.stop_editing_search(),
        (KeyCode::Esc, _) => app.clear_search(),
        (KeyCode::Backspace, _) => app.pop_search_char(),
        (KeyCode::Char('w'), KeyModifiers::CONTROL) => app.pop_search_word(),
        (KeyCode::Char('u'), KeyModifiers::CONTROL) => app.clear_search_text(),
        (KeyCode::Left, _) => move_left(app, columns),
        (KeyCode::Right, _) => move_right(app, columns),
        (KeyCode::Up, _) => app.move_up(columns),
        (KeyCode::Down, _) => app.move_down(columns),
        (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) => app.push_search_char(ch),
        _ => {}
    }
}

fn move_left(app: &mut App, columns: usize) {
    let columns = columns.max(1);
    if !app.selected_index.is_multiple_of(columns) {
        app.move_left();
    }
}

fn move_right(app: &mut App, columns: usize) {
    let columns = columns.max(1);
    if app.selected_index % columns != columns - 1 {
        app.move_right();
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::layout::Rect;

    use super::*;
    use crate::model::{App, Session, WorkspaceTarget};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn session(name: &str) -> Session {
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
    fn arrow_keys_move_selection() {
        let mut app = App::new(vec![session("one"), session("two"), session("three")], None);

        handle_key(&mut app, key(KeyCode::Right), 2);
        assert_eq!(app.selected_index, 1);

        handle_key(&mut app, key(KeyCode::Down), 2);
        assert_eq!(app.selected_index, 2);

        handle_key(&mut app, key(KeyCode::Left), 2);
        assert_eq!(app.selected_index, 2);
    }

    #[test]
    fn workspace_enter_opens_session_and_escape_returns_to_overview() {
        let mut app = App::new(vec![session("one"), session("two")], None);
        handle_key(&mut app, key(KeyCode::Right), 2);

        handle_key(&mut app, key(KeyCode::Enter), 2);
        assert_eq!(app.focus, Focus::Session);
        assert!(!app.should_switch);

        handle_key(&mut app, key(KeyCode::Esc), 2);
        assert_eq!(app.focus, Focus::Overview);
        assert!(!app.should_quit);
        assert_eq!(app.selected_session().unwrap().name, "two");
    }

    #[test]
    fn workspace_close_requires_confirmation_and_keeps_the_original_target() {
        let mut app = App::new(vec![session("one"), session("two")], None);
        let d = key(KeyCode::Char('d'));

        handle_key(&mut app, d, 2);
        assert!(app.dialog.is_some());
        assert!(app.confirmed_action.is_none());
        handle_key(&mut app, key(KeyCode::Esc), 2);
        assert!(app.dialog.is_none());
        assert!(app.confirmed_action.is_none());
        assert!(!app.should_quit);

        handle_key(&mut app, d, 2);
        app.replace_sessions(vec![session("two")]);
        handle_key(&mut app, key(KeyCode::Char('y')), 2);
        assert!(app.confirmed_action.is_none());
        handle_key(&mut app, key(KeyCode::Enter), 2);
        assert!(
            matches!(app.confirmed_action, Some(WorkspaceDialog::Close(WorkspaceTarget::Session { ref id, .. })) if id == "$one")
        );
    }

    #[test]
    fn workspace_filters_only_after_slash_and_can_rename_a_session() {
        let mut app = App::new(vec![session("alpha"), session("docs")], None);

        handle_key(&mut app, key(KeyCode::Char('o')), 2);
        assert!(app.search_text().is_none());
        handle_key(&mut app, key(KeyCode::Char('r')), 2);
        assert!(matches!(app.dialog, Some(WorkspaceDialog::Rename(..))));
        handle_key(&mut app, key(KeyCode::Enter), 2);
        assert!(app.confirmed_action.is_none());
        for ch in "new 项目".chars() {
            handle_key(&mut app, key(KeyCode::Char(ch)), 2);
        }
        handle_key(&mut app, key(KeyCode::Enter), 2);
        assert!(matches!(app.confirmed_action,
            Some(WorkspaceDialog::Rename(WorkspaceTarget::Session { ref id, .. }, ref name))
                if id == "$alpha" && name == "new 项目"));

        handle_key(&mut app, key(KeyCode::Char('/')), 2);
        handle_key(&mut app, key(KeyCode::Char('d')), 2);
        assert_eq!(app.search_text(), Some("d"));
        assert_eq!(app.selected_session().unwrap().name, "docs");
        assert!(app.dialog.is_none());
    }

    #[test]
    fn settings_click_selects_the_field_and_digits_replace_its_value() {
        let mut app = App::new(vec![session("one")], None);
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(','), KeyModifiers::CONTROL),
            1,
        );
        let frame = Rect::new(0, 0, 100, 40);
        let (panel, _) = ui::settings_panel(frame, 0, Page::Grid.fields());
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: panel.x + 3,
                row: panel.y + 4,
                modifiers: KeyModifiers::NONE,
            },
            Rect {
                height: 39,
                ..frame
            },
            None,
        );
        for ch in ['1', '2'] {
            handle_key(&mut app, key(KeyCode::Char(ch)), 1);
        }
        assert_eq!(
            app.grid_edit.as_ref().unwrap().settings().unwrap().fields(),
            [2, 2, 12, 2, 1]
        );
        handle_key(&mut app, key(KeyCode::Backspace), 1);
        handle_key(&mut app, key(KeyCode::Enter), 1);
        assert!(app.save_grid);
        assert_eq!(
            app.grid_edit
                .as_ref()
                .unwrap()
                .settings()
                .unwrap()
                .windows
                .columns,
            1
        );
    }

    #[test]
    fn alt_w_opens_the_attached_session_even_when_another_is_selected() {
        let mut app = App::new(
            vec![session("other"), session("attached")],
            Some("attached".into()),
        );
        app.selected_index = 0;
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::ALT),
            1,
        );
        assert_eq!(app.focus, Focus::Session);
        assert_eq!(app.selected_session().unwrap().name, "attached");
    }

    fn vim_app(names: &[&str]) -> App {
        let mut app = App::new(names.iter().map(|name| session(name)).collect(), None);
        app.vim_keys = true;
        app
    }

    #[test]
    fn vim_hjkl_move_selection_without_filtering() {
        let mut app = vim_app(&["one", "two", "three", "four"]);

        handle_key(&mut app, key(KeyCode::Char('l')), 2); // right
        assert_eq!(app.selected_index, 1);

        handle_key(&mut app, key(KeyCode::Char('j')), 2); // down
        assert_eq!(app.selected_index, 3);

        handle_key(&mut app, key(KeyCode::Char('h')), 2); // left
        assert_eq!(app.selected_index, 2);

        handle_key(&mut app, key(KeyCode::Char('k')), 2); // up
        assert_eq!(app.selected_index, 0);

        // hjkl must not leak into the search filter while navigating.
        assert!(!app.is_searching());
    }

    #[test]
    fn vim_arrows_still_move_selection() {
        let mut app = vim_app(&["one", "two", "three"]);

        handle_key(&mut app, key(KeyCode::Right), 2);
        assert_eq!(app.selected_index, 1);

        handle_key(&mut app, key(KeyCode::Down), 2);
        assert_eq!(app.selected_index, 2);
    }

    #[test]
    fn vim_slash_enters_search_mode() {
        let mut app = vim_app(&["one", "two"]);

        handle_key(&mut app, key(KeyCode::Char('/')), 1);

        assert!(app.is_searching());
        assert_eq!(app.search_text(), Some(""));
    }

    #[test]
    fn vim_typing_in_search_filters_including_hjkl() {
        let mut app = vim_app(&["backend", "frontend", "hjkl-box"]);

        handle_key(&mut app, key(KeyCode::Char('/')), 1);
        handle_key(&mut app, key(KeyCode::Char('h')), 1);

        assert_eq!(app.search_text(), Some("h"));
        assert_eq!(app.selected_session().unwrap().name, "hjkl-box");
    }

    #[test]
    fn vim_esc_in_search_returns_to_normal_without_quitting() {
        let mut app = vim_app(&["one", "two"]);

        handle_key(&mut app, key(KeyCode::Char('/')), 1);
        handle_key(&mut app, key(KeyCode::Char('o')), 1);
        handle_key(&mut app, key(KeyCode::Esc), 1);

        assert!(!app.is_searching());
        assert!(!app.should_quit);
    }

    #[test]
    fn vim_esc_on_typed_query_keeps_it_as_a_filter() {
        // The actual bug this exists to pin: Esc used to discard the query
        // outright. It should commit it instead, Telescope-style, so hjkl
        // can browse the filtered results afterward.
        let mut app = vim_app(&["backend", "database", "notes"]);

        handle_key(&mut app, key(KeyCode::Char('/')), 1);
        handle_key(&mut app, key(KeyCode::Char('d')), 1);
        handle_key(&mut app, key(KeyCode::Esc), 1);

        assert!(!app.is_searching());
        assert_eq!(app.search_text(), Some("d"));
        assert_eq!(app.visible_session_count(), 2); // backend, database (not notes)
    }

    #[test]
    fn vim_hjkl_navigates_a_filter_committed_by_esc() {
        let mut app = vim_app(&["backend", "database", "notes"]);

        handle_key(&mut app, key(KeyCode::Char('/')), 1);
        handle_key(&mut app, key(KeyCode::Char('d')), 1);
        handle_key(&mut app, key(KeyCode::Esc), 1);
        assert_eq!(app.selected_session().unwrap().name, "backend");

        // If Esc failed to leave text-entry, 'j' would filter instead of moving.
        handle_key(&mut app, key(KeyCode::Char('j')), 1);

        assert_eq!(app.search_text(), Some("d"));
        assert_eq!(app.selected_session().unwrap().name, "database");
    }

    #[test]
    fn vim_esc_on_empty_query_cancels_instead_of_committing() {
        let mut app = vim_app(&["one", "two"]);

        handle_key(&mut app, key(KeyCode::Char('/')), 1);
        handle_key(&mut app, key(KeyCode::Esc), 1);

        assert!(!app.is_searching());
        assert_eq!(app.search_text(), None);
        assert!(!app.should_quit);
    }

    #[test]
    fn vim_esc_on_committed_filter_clears_it_without_quitting() {
        let mut app = vim_app(&["backend", "frontend"]);

        handle_key(&mut app, key(KeyCode::Char('/')), 1);
        handle_key(&mut app, key(KeyCode::Char('f')), 1);
        handle_key(&mut app, key(KeyCode::Esc), 1); // commit filter, -> NORMAL

        handle_key(&mut app, key(KeyCode::Esc), 1); // first NORMAL Esc: clear filter
        assert_eq!(app.search_text(), None);
        assert!(!app.should_quit);
        assert_eq!(app.visible_session_count(), 2);

        handle_key(&mut app, key(KeyCode::Esc), 1);
        assert!(!app.should_quit);
    }

    #[test]
    fn vim_q_quits_immediately_even_with_a_committed_filter() {
        let mut app = vim_app(&["backend", "frontend"]);

        handle_key(&mut app, key(KeyCode::Char('/')), 1);
        handle_key(&mut app, key(KeyCode::Char('f')), 1);
        handle_key(&mut app, key(KeyCode::Esc), 1); // commit filter, -> NORMAL

        handle_key(&mut app, key(KeyCode::Char('q')), 1);

        assert!(app.should_quit);
    }

    #[test]
    fn vim_slash_after_committed_filter_starts_a_fresh_search() {
        let mut app = vim_app(&["backend", "frontend"]);

        handle_key(&mut app, key(KeyCode::Char('/')), 1);
        handle_key(&mut app, key(KeyCode::Char('f')), 1);
        handle_key(&mut app, key(KeyCode::Esc), 1); // commit filter, -> NORMAL

        handle_key(&mut app, key(KeyCode::Char('/')), 1);

        assert!(app.is_searching());
        assert_eq!(app.search_text(), Some(""));
    }

    #[test]
    fn vim_plain_letters_do_not_filter_in_normal_mode() {
        let mut app = vim_app(&["alpha", "beta"]);

        handle_key(&mut app, key(KeyCode::Char('a')), 1);

        assert!(!app.is_searching());
        assert_eq!(app.search_text(), None);
    }

    #[test]
    fn vim_q_quits_in_normal_mode() {
        let mut app = vim_app(&["queue"]);

        handle_key(&mut app, key(KeyCode::Char('q')), 1);

        assert!(app.should_quit);
        assert!(!app.is_searching());
    }

    #[test]
    fn esc_keeps_overview_open_and_q_quits_in_normal_mode() {
        let mut app = vim_app(&["one"]);

        handle_key(&mut app, key(KeyCode::Esc), 1);

        assert!(!app.should_quit);
        handle_key(&mut app, key(KeyCode::Char('q')), 1);
        assert!(app.should_quit);
    }

    #[test]
    fn vim_ctrl_c_still_quits() {
        // Ctrl-C is handled before the mode dispatch, so it works in vim mode too.
        let mut app = vim_app(&["one"]);

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            1,
        );

        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_c_marks_app_for_exit() {
        let mut app = App::new(vec![session("one")], None);

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            1,
        );

        assert!(app.should_quit);
    }

    #[test]
    fn alt_e_does_not_quit_without_configured_toggle_key() {
        let mut app = App::new(vec![session("one")], None);

        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::ALT),
            1,
        );

        assert!(!app.should_quit);
        assert!(!app.should_switch);
    }

    #[test]
    fn configured_alt_e_marks_app_for_exit() {
        let mut app = App::new(vec![session("one")], None);
        let toggle_key = ToggleKey::from_tmux_key("M-e");

        handle_key_with_toggle(
            &mut app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::ALT),
            1,
            toggle_key,
        );

        assert!(app.should_quit);
        assert!(!app.should_switch);
    }

    #[test]
    fn plain_toggle_key_is_typeable_while_searching() {
        let mut app = App::new(vec![session("session")], None);
        let toggle_key = ToggleKey::from_tmux_key("s");
        app.start_search();
        app.push_search_char('e');

        // `s` is the toggle key, but while searching it must filter, not quit.
        handle_key_with_toggle(
            &mut app,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
            1,
            toggle_key,
        );

        assert!(!app.should_quit);
        assert_eq!(app.search_text(), Some("es"));
    }

    #[test]
    fn uppercase_toggle_key_is_typeable_while_searching() {
        let mut app = App::new(vec![session("Editor")], None);
        // `@tmux-expose-key 'E'` yields a toggle of Char('E') + SHIFT.
        let toggle_key = ToggleKey::from_tmux_key("E");
        app.start_search();

        handle_key_with_toggle(
            &mut app,
            KeyEvent::new(KeyCode::Char('E'), KeyModifiers::SHIFT),
            1,
            toggle_key,
        );

        assert!(!app.should_quit);
        assert_eq!(app.search_text(), Some("E"));
    }

    #[test]
    fn plain_toggle_key_still_quits_when_not_searching() {
        let mut app = App::new(vec![session("one")], None);
        let toggle_key = ToggleKey::from_tmux_key("s");

        handle_key_with_toggle(
            &mut app,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
            1,
            toggle_key,
        );

        assert!(app.should_quit);
    }

    #[test]
    fn modified_toggle_key_still_quits_while_searching() {
        let mut app = App::new(vec![session("one")], None);
        let toggle_key = ToggleKey::from_tmux_key("M-e");
        app.start_search();
        app.push_search_char('o');

        // A modified chord can't be typed into the filter, so it still closes.
        handle_key_with_toggle(
            &mut app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::ALT),
            1,
            toggle_key,
        );

        assert!(app.should_quit);
    }

    #[test]
    fn workspace_detail_selects_windows_and_switches_only_on_enter() {
        use crate::model::{PanePreview, WindowPreview};
        let mut work = session("work");
        let pane = |id: &str, index, left| PanePreview {
            id: id.into(),
            index,
            left,
            top: 0,
            width: 40,
            height: 20,
            active: index == 0,
            command: "sh".into(),
        };
        work.windows.push(WindowPreview {
            id: "@1".into(),
            index: 0,
            name: "shell".into(),
            width: 80,
            height: 20,
            active: true,
            panes: vec![pane("%1", 0, 0), pane("%2", 1, 40)],
        });
        work.windows.push(WindowPreview {
            id: "@2".into(),
            index: 1,
            name: "logs".into(),
            width: 40,
            height: 20,
            active: false,
            panes: vec![pane("%3", 0, 0)],
        });
        let mut app = App::new(vec![work], None);
        let selected = |app: &App| app.selected_window().unwrap().id.clone();
        handle_key(&mut app, key(KeyCode::Char('/')), 1);
        handle_key(&mut app, key(KeyCode::Char('w')), 1);
        handle_key(&mut app, key(KeyCode::Enter), 1);
        assert_eq!(app.focus, Focus::Session);
        assert!(!app.is_searching());
        assert_eq!(selected(&app), "@1");
        handle_key(&mut app, key(KeyCode::Tab), 1);
        assert_eq!(selected(&app), "@2");
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            1,
        );
        assert_eq!(selected(&app), "@1");

        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 33,
            row: 8,
            modifiers: KeyModifiers::NONE,
        };
        handle_mouse(&mut app, mouse, Rect::new(0, 0, 42, 14), Some(1));
        assert_eq!(selected(&app), "@2");
        assert!(!app.should_switch);

        let mut reordered = app.sessions.clone();
        reordered[0].windows.swap(0, 1);
        app.replace_sessions(reordered);
        assert_eq!(selected(&app), "@2");
        assert_eq!(app.search_text(), Some("w"));
        assert_eq!(app.focus, Focus::Session);
        handle_key(&mut app, key(KeyCode::Char('d')), 1);
        assert!(
            matches!(app.dialog, Some(WorkspaceDialog::Close(WorkspaceTarget::Window { ref id, ref name }))
            if id == "@2" && name == "work:1 (logs)")
        );
        handle_key(&mut app, key(KeyCode::Esc), 1);
        handle_key(&mut app, key(KeyCode::Enter), 1);
        assert!(app.should_switch);
    }

    #[test]
    fn characters_continue_filtering_while_searching() {
        let mut app = App::new(
            vec![session("backend"), session("frontend"), session("database")],
            None,
        );

        handle_key(&mut app, key(KeyCode::Char('/')), 1);
        handle_key(&mut app, key(KeyCode::Char('f')), 1);
        handle_key(&mut app, key(KeyCode::Char('r')), 1);

        assert_eq!(app.search_text(), Some("fr"));
        assert_eq!(app.visible_session_count(), 1);
        assert_eq!(app.selected_session().unwrap().name, "frontend");
    }

    #[test]
    fn esc_clears_search_before_quitting() {
        let mut app = App::new(vec![session("one")], None);

        handle_key(&mut app, key(KeyCode::Char('/')), 1);
        handle_key(&mut app, key(KeyCode::Char('o')), 1);
        handle_key(&mut app, key(KeyCode::Esc), 1);

        assert!(!app.is_searching());
        assert!(!app.should_quit);
    }

    #[test]
    fn backspace_edits_search_query() {
        let mut app = App::new(vec![session("frontend")], None);

        handle_key(&mut app, key(KeyCode::Char('/')), 1);
        handle_key(&mut app, key(KeyCode::Char('f')), 1);
        handle_key(&mut app, key(KeyCode::Backspace), 1);

        assert_eq!(app.search_text(), Some(""));
    }

    fn type_query(app: &mut App, text: &str) {
        handle_key(app, key(KeyCode::Char('/')), 1);
        for ch in text.chars() {
            handle_key(app, key(KeyCode::Char(ch)), 1);
        }
    }

    #[test]
    fn ctrl_w_deletes_previous_word_from_search_query() {
        let mut app = App::new(vec![session("frontend")], None);

        type_query(&mut app, "api dev ");
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
            1,
        );

        assert_eq!(app.search_text(), Some("api "));
    }

    #[test]
    fn ctrl_u_clears_search_query_but_keeps_searching() {
        let mut app = App::new(vec![session("frontend")], None);

        type_query(&mut app, "api dev");
        handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            1,
        );

        assert_eq!(app.search_text(), Some(""));
        assert!(app.is_searching());
    }

    #[test]
    fn horizontal_navigation_clamps_at_row_edges() {
        let mut app = App::new(
            vec![
                session("one"),
                session("two"),
                session("three"),
                session("four"),
            ],
            None,
        );
        app.selected_index = 2;

        handle_key(&mut app, key(KeyCode::Right), 3);
        assert_eq!(app.selected_index, 2);

        app.selected_index = 3;
        handle_key(&mut app, key(KeyCode::Left), 3);
        assert_eq!(app.selected_index, 3);
    }
}
