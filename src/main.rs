use std::{
    env, io,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::Parser;
use crossterm::{
    cursor::{Hide, Show},
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::style::Color;
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};
use tmux_expose::{
    appearance::{
        ACTIVE_REFRESH, AGENT_SORT, Appearance, MAX_CAPTURES, PANE_REFRESH, TOPOLOGY_REFRESH,
        parse_color,
    },
    grid::Settings,
    input,
    model::{self, AgentStatus, App},
    preview::{PreviewCache, RefreshPolicy},
    tmux, ui,
};

mod popup;

#[derive(Debug, Parser)]
#[command(version, about = "Mission Control-style tmux session switcher")]
struct Cli {
    #[arg(long, default_value_t = 1500, value_name = "MS", value_parser = clap::value_parser!(u64).range(1..))]
    topology_refresh: u64,

    #[arg(long, default_value_t = 1500, value_name = "MS", value_parser = clap::value_parser!(u64).range(1..))]
    pane_refresh: u64,

    #[arg(long, default_value_t = 300, value_name = "MS", value_parser = clap::value_parser!(u64).range(1..))]
    active_pane_refresh: u64,

    #[arg(long, default_value_t = 4, value_name = "N", value_parser = clap::value_parser!(u16).range(1..=32))]
    max_captures: u16,

    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u16).range(1..))]
    columns: Option<u16>,

    #[arg(long, value_name = "COLOR", value_parser = parse_color)]
    selected_color: Option<Color>,

    #[arg(long, value_name = "COLOR", value_parser = parse_color)]
    attached_color: Option<Color>,

    #[arg(long, value_name = "COLOR", value_parser = parse_color)]
    inactive_color: Option<Color>,

    #[arg(long, value_name = "COLOR", value_parser = parse_color)]
    attention_color: Option<Color>,

    #[arg(long, value_name = "COLOR", value_parser = parse_color)]
    waiting_color: Option<Color>,

    #[arg(long, value_name = "COLOR", value_parser = parse_color)]
    working_color: Option<Color>,

    /// Use modal vim navigation: hjkl to move, `/` to search, q/Esc to quit.
    #[arg(long)]
    vim: bool,

    /// Keep tmux's own session order instead of sorting sessions with a
    /// waiting agent to the top.
    #[arg(long)]
    no_agent_sort: bool,
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("failed to enable raw mode")?;
        let guard = Self;
        execute!(io::stdout(), EnterAlternateScreen, Hide, EnableMouseCapture)
            .context("failed to enter alternate screen")?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            Show,
            LeaveAlternateScreen,
            DisableMouseCapture
        );
    }
}

/// One-shot side-effecting command meant to be wired straight into an
/// agent's own hook config (e.g. Claude Code's `settings.json`) as
/// `tmux-expose agent-status <working|waiting|attention|clear>`. Deliberately
/// not a clap subcommand: it never launches the TUI, so hook configs only
/// need to know one binary name, and it can't collide with the picker's own
/// flags.
///
/// Records status on the calling pane (via `$TMUX_PANE`, which tmux injects
/// into every pane's shell — and which a hook subprocess inherits, since
/// it's a child of that same shell) so tmux-expose can read it back later.
fn run_agent_status(status: &str) -> Result<()> {
    validate_agent_status_word(status)?;

    let Ok(pane) = env::var("TMUX_PANE") else {
        return Ok(()); // Not inside tmux — nothing to record.
    };
    if pane.is_empty() {
        return Ok(());
    }

    if status == "clear" {
        return tmux::write_agent_status(&pane, None);
    }
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time precedes the Unix epoch")?
        .as_secs()
        .to_string();
    tmux::write_agent_status(&pane, Some((status, &since)))
}

fn validate_agent_status_word(status: &str) -> Result<()> {
    if status == "clear" || AgentStatus::parse(status).is_some() {
        return Ok(());
    }
    bail!("unknown agent status {status:?}; expected working, waiting, attention, or clear");
}

fn main() -> Result<()> {
    let mut raw_args = env::args();
    raw_args.next(); // program name
    if raw_args.next().as_deref() == Some("agent-status") {
        return run_agent_status(&raw_args.next().unwrap_or_default());
    }

    let cli = Cli::parse();
    let mut previews = PreviewCache::new(env::var("TMUX_PANE").ok());
    let configured = configured_appearance(&cli);
    let (appearance, config_error) = match Appearance::load(configured.clone()) {
        Ok(appearance) => (appearance, None),
        Err(error) => (configured, Some(error.to_string())),
    };

    let client = env::var("TMUX_EXPOSE_CLIENT").ok();
    let current_session_id = env::var("TMUX_EXPOSE_SESSION")
        .ok()
        .or_else(|| tmux::current_session_id().unwrap_or(None));
    let agent_sort = appearance.0[AGENT_SORT] == "on";
    let mut app = match tmux::list_workspace_sessions() {
        Ok(mut sessions) => {
            let current_session_name = sessions
                .iter()
                .find(|session| Some(session.id.as_str()) == current_session_id.as_deref())
                .map(|session| session.name.clone());
            if agent_sort {
                model::sort_sessions_by_agent_status_with_current(
                    &mut sessions,
                    current_session_name.as_deref(),
                );
            }
            let mut app = App::new(sessions, current_session_name);
            if agent_sort {
                app.select_most_urgent_agent_session();
            }
            app
        }
        Err(error) => {
            let mut app = App::new(Vec::new(), None);
            app.error = Some(error.to_string());
            app
        }
    };
    app.appearance = appearance;
    app.grid_error = config_error;
    app.vim_keys = app.appearance.0[7] == "on";
    match Settings::load() {
        Ok(settings) => app.grid = settings,
        Err(error) => app.grid_error = Some(error.to_string()),
    }
    let mut colors = appearance_colors(&app.appearance);
    let mut popup_resize = popup::Resize::from_env(client.as_deref(), &mut app);

    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).context("failed to create terminal")?;

    let toggle_key = env::var("TMUX_EXPOSE_TOGGLE_KEY")
        .ok()
        .and_then(|key| input::ToggleKey::from_tmux_key(&key));
    let mut last_refresh = Instant::now();
    let mut topology_available = app.error.is_none();

    loop {
        let forced_columns = cli.columns.map(usize::from);
        let grid_area = current_grid_area(&terminal)?;
        let viewport = ui::workspace_viewport(&app, grid_area, forced_columns);
        match app.focus {
            model::Focus::Overview => app.session_row = viewport.first_row,
            model::Focus::Session => app.window_row = viewport.first_row,
        }
        terminal.draw(|frame| ui::render_view(frame, &app, colors, forced_columns, &previews))?;

        if app.should_quit {
            break;
        }

        if app.should_switch {
            match switch_selection(&app, client.as_deref()) {
                Ok(true) => break,
                Ok(false) => {}
                Err(error) => {
                    app.error = Some(error.to_string());
                }
            }
            app.should_switch = false;
        }

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    input::handle_key_with_toggle(&mut app, key, viewport.columns, toggle_key);
                }
                Event::Mouse(mouse) => {
                    let grid_area = current_grid_area(&terminal)?;
                    input::handle_mouse(&mut app, mouse, grid_area, forced_columns);
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }

        let mut settings_saved = false;
        if std::mem::take(&mut app.save_grid) {
            match save_settings(&mut app) {
                Ok(()) => {
                    colors = appearance_colors(&app.appearance);
                    settings_saved = true;
                }
                Err(error) => app.grid_error = Some(format!("{error:#}")),
            }
        }
        let mut changed_target = false;
        if let Some(action) = app.confirmed_action.take() {
            match tmux::apply_workspace_action(action) {
                Ok(()) => changed_target = true,
                Err(error) => {
                    app.error = Some(error.to_string());
                    last_refresh = Instant::now();
                }
            }
        }
        let refresh_topology = !app.should_quit
            && !app.should_switch
            && (changed_target
                || settings_saved
                || last_refresh.elapsed()
                    >= Duration::from_millis(app.appearance.number(TOPOLOGY_REFRESH)));
        if refresh_topology {
            match tmux::list_workspace_sessions() {
                Ok(mut sessions) => {
                    app.current_session_name = sessions
                        .iter()
                        .find(|session| Some(session.id.as_str()) == current_session_id.as_deref())
                        .map(|session| session.name.clone());
                    topology_available = true;
                    previews.reconcile(&sessions);
                    if app.appearance.0[AGENT_SORT] == "on" {
                        model::sort_sessions_by_agent_status_with_current(
                            &mut sessions,
                            app.current_session_name.as_deref(),
                        );
                    }
                    app.replace_sessions(sessions);
                    app.error = None;
                }
                Err(error) => {
                    topology_available = false;
                    app.error = Some(error.to_string());
                }
            }
            last_refresh = Instant::now();
        }
        if !app.should_quit
            && !app.should_switch
            && let Some(resize) = &mut popup_resize
            && resize.reopen(&app)
        {
            drop(terminal);
            drop(_guard);
            std::process::exit(75);
        }
        if topology_available && !app.should_quit && !app.should_switch {
            let visible = app.visible_sessions();
            let selected = app.selected_session();
            let active_pane = app
                .selected_window()
                .and_then(|window| window.panes.iter().find(|pane| pane.active));
            previews.refresh(
                &visible,
                selected.map(|session| session.id.as_str()),
                active_pane.map(|pane| pane.id.as_str()),
                RefreshPolicy {
                    selected_pane: Duration::from_millis(app.appearance.number(ACTIVE_REFRESH)),
                    background: Duration::from_millis(app.appearance.number(PANE_REFRESH)),
                    max_captures: app.appearance.number(MAX_CAPTURES) as usize,
                    ..RefreshPolicy::default()
                },
            );
        }
    }

    Ok(())
}

fn configured_appearance(cli: &Cli) -> Appearance {
    let mut appearance = Appearance::from_tmux();
    if cli.vim {
        appearance.0[7] = "on".into();
    }
    if cli.no_agent_sort {
        appearance.0[AGENT_SORT] = "off".into();
    }
    appearance.0[TOPOLOGY_REFRESH] = cli.topology_refresh.to_string();
    appearance.0[PANE_REFRESH] = cli.pane_refresh.to_string();
    appearance.0[ACTIVE_REFRESH] = cli.active_pane_refresh.to_string();
    appearance.0[MAX_CAPTURES] = cli.max_captures.to_string();
    for (index, color) in [
        cli.selected_color,
        cli.attached_color,
        cli.inactive_color,
        cli.attention_color,
        cli.waiting_color,
        cli.working_color,
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(color) = color {
            appearance.0[8 + index] = color.to_string();
        }
    }
    appearance
}

fn appearance_colors(appearance: &Appearance) -> ui::CardColors {
    let defaults = ui::CardColors::default();
    let color = |index: usize, fallback| parse_color(&appearance.0[index]).unwrap_or(fallback);
    ui::CardColors {
        selected: color(8, defaults.selected),
        attached: color(9, defaults.attached),
        inactive: color(10, defaults.inactive),
        attention: color(11, defaults.attention),
        waiting: color(12, defaults.waiting),
        working: color(13, defaults.working),
    }
}

fn save_settings(app: &mut App) -> Result<()> {
    let lang = app.language();
    let grid = app
        .grid_edit
        .as_ref()
        .and_then(|edit| edit.settings())
        .context(lang.text(
            "行列范围为 1–12，间距范围为 0–3",
            "Use 1–12 for rows and columns and 0–3 for card gap.",
        ))?;
    let appearance = &app
        .appearance_edit
        .as_ref()
        .context(lang.text("设置已关闭", "Settings are closed"))?
        .draft;
    let changed_appearance = *appearance != app.appearance;
    let changed_grid = grid != app.grid;
    if changed_grid || !changed_appearance {
        grid.save()
            .context(lang.text("保存网格失败", "Could not save grid settings"))?;
    }
    if changed_appearance && let Err(error) = appearance.save(&app.appearance) {
        if changed_grid {
            app.grid
                .save()
                .context(lang.text("恢复网格设置失败", "Could not restore grid settings"))?;
        }
        return Err(error);
    }
    app.grid = grid;
    app.appearance = appearance.clone();
    app.vim_keys = app.appearance.0[7] == "on";
    app.close_settings();
    Ok(())
}

fn switch_selection(app: &App, client: Option<&str>) -> Result<bool> {
    let Some(session) = app.selected_session() else {
        return Ok(false);
    };
    let Some(window) = app.selected_window() else {
        return Ok(false);
    };
    tmux::switch_client_for(client, &format!("{}:{}", session.id, window.id))?;
    Ok(true)
}

fn current_grid_area(terminal: &Terminal<CrosstermBackend<io::Stdout>>) -> Result<Rect> {
    let area = terminal.size().context("failed to read terminal size")?;
    Ok(Rect::new(0, 0, area.width, area.height.saturating_sub(1)))
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn parses_forced_columns_option() {
        let cli = Cli::parse_from(["tmux-expose", "--columns", "2"]);

        assert_eq!(cli.columns, Some(2));
    }

    #[test]
    fn color_options_default_to_none() {
        let cli = Cli::parse_from(["tmux-expose"]);

        assert_eq!(cli.selected_color, None);
        assert_eq!(cli.attached_color, None);
        assert_eq!(cli.inactive_color, None);
    }

    #[test]
    fn parses_named_color() {
        let cli = Cli::parse_from(["tmux-expose", "--selected-color", "cyan"]);

        assert_eq!(cli.selected_color, Some(Color::Cyan));
    }

    #[test]
    fn parses_indexed_color() {
        let cli = Cli::parse_from(["tmux-expose", "--attached-color", "208"]);

        assert_eq!(cli.attached_color, Some(Color::Indexed(208)));
    }

    #[test]
    fn parses_all_color_options() {
        let cli = Cli::parse_from([
            "tmux-expose",
            "--selected-color",
            "magenta",
            "--attached-color",
            "green",
            "--inactive-color",
            "white",
        ]);

        assert_eq!(cli.selected_color, Some(Color::Magenta));
        assert_eq!(cli.attached_color, Some(Color::Green));
        assert_eq!(cli.inactive_color, Some(Color::White));
    }

    #[test]
    fn parses_hex_color() {
        let cli = Cli::parse_from(["tmux-expose", "--selected-color", "#ff8700"]);

        assert_eq!(cli.selected_color, Some(Color::Rgb(255, 135, 0)));
    }

    #[test]
    fn parses_tmux_style_indexed_color() {
        // tmux spells indexed colors as `colour208`; ratatui only accepts `208`.
        assert_eq!(parse_color("colour208"), Ok(Color::Indexed(208)));
        assert_eq!(parse_color("color208"), Ok(Color::Indexed(208)));
        assert_eq!(parse_color("208"), Ok(Color::Indexed(208)));
    }

    #[test]
    fn rejects_invalid_color() {
        let result = Cli::try_parse_from(["tmux-expose", "--selected-color", "not-a-color"]);

        assert!(result.is_err());
        // `colour` prefix with a non-numeric / out-of-range suffix is still invalid.
        assert!(parse_color("colourize").is_err());
        assert!(parse_color("colour999").is_err());
    }

    #[test]
    fn vim_defaults_to_off() {
        let cli = Cli::parse_from(["tmux-expose"]);

        assert!(!cli.vim);
    }

    #[test]
    fn parses_vim_flag() {
        let cli = Cli::parse_from(["tmux-expose", "--vim"]);

        assert!(cli.vim);
    }

    #[test]
    fn agent_status_colors_default_to_none() {
        let cli = Cli::parse_from(["tmux-expose"]);

        assert_eq!(cli.attention_color, None);
        assert_eq!(cli.waiting_color, None);
        assert_eq!(cli.working_color, None);
    }

    #[test]
    fn parses_agent_status_colors() {
        let cli = Cli::parse_from([
            "tmux-expose",
            "--attention-color",
            "red",
            "--waiting-color",
            "colour208",
            "--working-color",
            "#8be9fd",
        ]);

        assert_eq!(cli.attention_color, Some(Color::Red));
        assert_eq!(cli.waiting_color, Some(Color::Indexed(208)));
        assert_eq!(cli.working_color, Some(Color::Rgb(139, 233, 253)));
    }

    #[test]
    fn agent_sort_is_on_by_default() {
        let cli = Cli::parse_from(["tmux-expose"]);

        assert!(!cli.no_agent_sort);
    }

    #[test]
    fn parses_no_agent_sort_flag() {
        let cli = Cli::parse_from(["tmux-expose", "--no-agent-sort"]);

        assert!(cli.no_agent_sort);
    }

    #[test]
    fn accepts_known_agent_status_words() {
        for word in ["working", "waiting", "attention", "clear"] {
            assert!(
                validate_agent_status_word(word).is_ok(),
                "{word} should be valid"
            );
        }
    }

    #[test]
    fn rejects_unknown_agent_status_words() {
        assert!(validate_agent_status_word("").is_err());
        assert!(validate_agent_status_word("done").is_err());
        assert!(validate_agent_status_word("Waiting").is_err());
    }
}
