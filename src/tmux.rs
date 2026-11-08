use std::collections::HashMap;
use std::process::{Command, Output};

use anyhow::{Context, Result, anyhow};

use crate::model::{AgentPaneCounts, AgentStatus, PanePreview, Session, WindowPreview};

const FIELD_SEPARATOR: char = ':';
const LEGACY_FIELD_SEPARATOR: char = '\u{1f}';
// tmux escapes control characters in -F output (US becomes literal "\\037").
// A printable private-use character keeps fields separate without colliding
// with ordinary punctuation in session and window names.
const WORKSPACE_SEPARATOR: char = '\u{e000}';
// Keep each tmux format and its decoded fields in the same declaration.
macro_rules! tmux_fields {
    ($format:ident, $record:ident { $first:ident: $first_expr:literal $(, $field:ident: $expr:literal)* $(,)? }) => {
        const $format: &str = concat!($first_expr $(, "\u{e000}", $expr)*);

        struct $record<'a> {
            $first: &'a str,
            $($field: &'a str,)*
        }

        impl<'a> $record<'a> {
            fn parse(line: &'a str) -> Result<Self> {
                let mut parts = line.split(WORKSPACE_SEPARATOR);
                let result = Self {
                    $first: parts.next().context(concat!("missing ", stringify!($first)))?,
                    $($field: parts.next().context(concat!("missing ", stringify!($field)))?,)*
                };
                anyhow::ensure!(parts.next().is_none(), "unexpected extra field");
                Ok(result)
            }
        }
    };
}

tmux_fields!(
    SESSION_FORMAT,
    SessionFields {
        id: "#{session_id}",
        name: "#{session_name}",
        attached: "#{session_attached}",
        window_count: "#{session_windows}",
    }
);

tmux_fields!(
    WORKSPACE_PANE_FORMAT,
    WorkspaceFields {
        session_id: "#{session_id}",
        window_id: "#{window_id}",
        window_index: "#{window_index}",
        window_name: "#{window_name}",
        window_width: "#{window_width}",
        window_height: "#{window_height}",
        window_active: "#{window_active}",
        pane_id: "#{pane_id}",
        pane_index: "#{pane_index}",
        pane_left: "#{pane_left}",
        pane_top: "#{pane_top}",
        pane_width: "#{pane_width}",
        pane_height: "#{pane_height}",
        pane_active: "#{pane_active}",
        pane_command: "#{pane_current_command}",
        agent_status: "#{@agent_status}",
        agent_status_since: "#{@agent_status_since}",
        zoomed_layout: "#{?window_zoomed_flag,#{window_layout},}",
        // A retained dead pane keeps its options, even after respawn-pane.
        stale: "#{||:#{pane_dead},#{&&:#{@agent_status_pane_pid},#{!=:#{pane_pid},#{@agent_status_pane_pid}}}}",
    }
);

type PaneAgentRow = (String, Option<AgentStatus>, Option<i64>);

#[derive(Debug)]
struct WorkspaceRow {
    session_id: String,
    window: WindowPreview,
    pane: PanePreview,
    agent_status: Option<AgentStatus>,
    agent_status_since: Option<i64>,
}

fn parse_rows<T>(output: &str, command: &str, parse: impl Fn(&str) -> Result<T>) -> Result<Vec<T>> {
    output
        .lines()
        .enumerate()
        .map(|(index, line)| {
            parse(line).map_err(|error| {
                anyhow!(
                    "unexpected format from tmux {command} on row {}: {error:#}",
                    index + 1
                )
            })
        })
        .collect()
}

fn parse_workspace_rows(output: &str) -> Result<Vec<WorkspaceRow>> {
    parse_rows(output, "list-panes", parse_workspace_row)
}

fn parse_workspace_row(line: &str) -> Result<WorkspaceRow> {
    let fields = WorkspaceFields::parse(line)?;
    anyhow::ensure!(!fields.session_id.is_empty(), "missing session_id");
    anyhow::ensure!(!fields.window_id.is_empty(), "missing window_id");
    anyhow::ensure!(!fields.pane_id.is_empty(), "missing pane_id");
    let (agent_status, agent_status_since) = if fields.stale == "1" {
        (None, None)
    } else {
        (
            AgentStatus::parse(fields.agent_status),
            fields.agent_status_since.parse().ok(),
        )
    };
    let mut row = WorkspaceRow {
        session_id: fields.session_id.to_owned(),
        window: WindowPreview {
            id: fields.window_id.to_owned(),
            index: fields
                .window_index
                .parse()
                .context("invalid window_index")?,
            name: fields.window_name.to_owned(),
            width: fields
                .window_width
                .parse()
                .context("invalid window_width")?,
            height: fields
                .window_height
                .parse()
                .context("invalid window_height")?,
            active: fields.window_active == "1",
            panes: Vec::new(),
        },
        pane: PanePreview {
            id: fields.pane_id.to_owned(),
            index: fields.pane_index.parse().context("invalid pane_index")?,
            left: fields.pane_left.parse().context("invalid pane_left")?,
            top: fields.pane_top.parse().context("invalid pane_top")?,
            width: fields.pane_width.parse().context("invalid pane_width")?,
            height: fields.pane_height.parse().context("invalid pane_height")?,
            active: fields.pane_active == "1",
            command: fields.pane_command.to_owned(),
        },
        agent_status,
        agent_status_since,
    };
    if !fields.zoomed_layout.is_empty() {
        restore_unzoomed_geometry(fields.zoomed_layout, &mut row.window, &mut row.pane)
            .context("invalid zoomed window layout")?;
    }
    Ok(row)
}

fn restore_unzoomed_geometry(
    layout: &str,
    window: &mut WindowPreview,
    pane: &mut PanePreview,
) -> Option<()> {
    // tmux layout strings start with a checksum, then a tree of rectangles.
    // A leaf has a numeric pane ID after its size, x and y. Split branches
    // have a child rectangle there instead.
    let (_, tree) = layout.split_once(',')?;
    let fields: Vec<_> = tree.split([',', '{', '}', '[', ']']).collect();
    (window.width, window.height) = layout_size(fields.first()?)?;
    restore_pane_geometry(&fields, pane)
}

fn restore_pane_geometry(fields: &[&str], pane: &mut PanePreview) -> Option<()> {
    let pane_id = pane.id.strip_prefix('%')?;
    let leaf = fields
        .windows(4)
        .find(|fields| fields[0].contains('x') && fields[3] == pane_id)?;
    (pane.width, pane.height) = layout_size(leaf[0])?;
    pane.left = leaf[1].parse().ok()?;
    pane.top = leaf[2].parse().ok()?;
    Some(())
}

fn layout_size(size: &str) -> Option<(u16, u16)> {
    let (width, height) = size.split_once('x')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

fn attach_workspace_rows(sessions: &mut [Session], rows: &[WorkspaceRow]) {
    let agent_rows: Vec<_> = rows
        .iter()
        .map(|row| {
            (
                row.session_id.clone(),
                row.agent_status,
                row.agent_status_since,
            )
        })
        .collect();
    apply_agent_status(sessions, &agent_rows);

    for row in rows {
        let Some(session) = sessions
            .iter_mut()
            .find(|session| session.id == row.session_id)
        else {
            continue;
        };
        let window = if let Some(index) = session
            .windows
            .iter()
            .position(|window| window.id == row.window.id)
        {
            &mut session.windows[index]
        } else {
            session.windows.push(row.window.clone());
            session.windows.last_mut().unwrap()
        };
        if window.panes.iter().any(|pane| pane.id == row.pane.id) {
            continue;
        }
        window.panes.push(row.pane.clone());
    }
    for session in sessions {
        session.windows.sort_by_key(|window| window.index);
        for window in &mut session.windows {
            window.panes.sort_by_key(|pane| pane.index);
        }
    }
}

pub fn list_workspace_sessions() -> Result<Vec<Session>> {
    workspace_snapshot(false).map(|(sessions, _)| sessions)
}

pub fn workspace_snapshot(read_current: bool) -> Result<(Vec<Session>, Option<String>)> {
    let mut args = Vec::new();
    if read_current {
        args.extend(["display-message", "-p", "#{session_id}", ";"]);
    }
    args.extend([
        "list-sessions",
        "-F",
        SESSION_FORMAT,
        ";",
        "display-message",
        "-p",
        WORKSPACE_BOUNDARY.trim_end_matches('\n'),
        ";",
        "list-panes",
        "-a",
        "-F",
        WORKSPACE_PANE_FORMAT,
    ]);
    let output = run_tmux(&args, "tmux workspace query failed")?;
    let (current, body) = if read_current {
        let (current, body) = output
            .split_once('\n')
            .context("missing current session in tmux snapshot")?;
        ((!current.is_empty()).then(|| current.to_owned()), body)
    } else {
        (None, output.as_str())
    };
    Ok((parse_workspace_snapshot(body)?, current))
}

const WORKSPACE_BOUNDARY: &str = "\u{e000}tmux-expose-panes\u{e000}\n";

fn parse_workspace_snapshot(output: &str) -> Result<Vec<Session>> {
    let (sessions, panes) = output
        .split_once(WORKSPACE_BOUNDARY)
        .context("missing separator in tmux workspace snapshot")?;
    let mut sessions = parse_sessions_output(sessions)?;
    let rows = parse_workspace_rows(panes)?;
    attach_workspace_rows(&mut sessions, &rows);
    if sessions
        .iter()
        .any(|session| session.windows.len() != session.window_count as usize)
    {
        return Err(anyhow!("tmux workspace topology changed during refresh"));
    }
    Ok(sessions)
}

pub fn write_agent_status(pane: &str, marker: Option<(&str, &str)>) -> Result<()> {
    let options = [
        ("@agent_status", marker.map(|(status, _)| status), "-p"),
        ("@agent_status_since", marker.map(|(_, since)| since), "-p"),
        (
            "@agent_status_pane_pid",
            marker.map(|_| "#{pane_pid}"),
            "-pF",
        ),
    ];
    let mut args = Vec::new();
    for (option, value, set_flag) in options {
        if !args.is_empty() {
            args.push(";");
        }
        args.extend([
            "set-option",
            if value.is_some() { set_flag } else { "-pu" },
            "-t",
            pane,
            option,
        ]);
        args.extend(value);
    }
    run_tmux(
        &args,
        &format!("failed to update agent status on tmux pane '{pane}'"),
    )?;
    Ok(())
}

pub fn current_session_id() -> Result<Option<String>> {
    let output = tmux_output(
        &["display-message", "-p", "#{session_id}"],
        "failed to run tmux display-message",
    )?;
    let id = String::from_utf8_lossy(&output.stdout);
    let id = id.trim();
    Ok((output.status.success() && !id.is_empty()).then(|| id.to_owned()))
}

pub fn capture_pane_preview(pane_id: &str) -> Result<Vec<String>> {
    let output = run_tmux(
        &["capture-pane", "-e", "-p", "-t", pane_id],
        &format!("failed to capture tmux pane '{pane_id}'"),
    )?;
    Ok(output.lines().map(str::to_owned).collect())
}

pub fn apply_workspace_action(action: crate::model::WorkspaceDialog) -> Result<()> {
    use crate::model::{WorkspaceDialog, WorkspaceTarget};

    let (target, name) = match &action {
        WorkspaceDialog::Close(target) => (target, None),
        WorkspaceDialog::Rename(target, name) => (target, Some(name)),
    };
    let (command, id) = match (target, name) {
        (WorkspaceTarget::Session { id, .. }, None) => ("kill-session", id),
        (WorkspaceTarget::Window { id, .. }, None) => ("kill-window", id),
        (WorkspaceTarget::Session { id, .. }, Some(_)) => ("rename-session", id),
        (WorkspaceTarget::Window { id, .. }, Some(_)) => ("rename-window", id),
    };
    let mut args = vec![command, "-t", id];
    let literal;
    if let Some(name) = name {
        if name.contains(WORKSPACE_SEPARATOR) {
            return Err(anyhow!("Name contains an unsupported character"));
        }
        // tmux expands formats in names and treats a final semicolon as a command separator.
        let mut escaped = name.replace('#', "##");
        if escaped.ends_with(';') {
            escaped.insert(escaped.len() - 1, '\\');
        }
        literal = escaped;
        args.extend(["--", &literal]);
    }
    run_tmux(
        &args,
        &format!("{command} failed for {}", target.description()),
    )?;
    Ok(())
}

/// Switches the client that opened the popup.
pub fn switch_client_for(client: Option<&str>, session_target: &str) -> Result<()> {
    let mut args = vec!["switch-client"];
    if let Some(client) = client {
        args.extend(["-c", client]);
    }
    args.extend(["-t", session_target]);
    run_tmux(
        &args,
        &format!("tmux switch-client failed for '{session_target}'"),
    )?;
    Ok(())
}

/// Parses session listings, including the legacy separators accepted by this library API.
pub fn parse_sessions(output: &str) -> Vec<Session> {
    output
        .lines()
        .filter_map(|line| {
            let separator = if line.contains(WORKSPACE_SEPARATOR) {
                WORKSPACE_SEPARATOR
            } else if line.contains(LEGACY_FIELD_SEPARATOR) {
                LEGACY_FIELD_SEPARATOR
            } else {
                FIELD_SEPARATOR
            };
            let mut parts = line.split(separator);
            let id = parts.next()?;
            let name = parts.next()?;
            if name.is_empty() {
                return None;
            }

            let attached = parts.next().is_some_and(|value| value != "0");
            let window_count = parts
                .next()
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0);
            Some(session_metadata(id, name, attached, window_count))
        })
        .collect()
}

/// Folds a session's panes down to one badge: the worst status present, and
/// how long the oldest pane at that status has held it.
#[derive(Default)]
struct AgentAggregate {
    counts: AgentPaneCounts,
    attention_since: Option<i64>,
    waiting_since: Option<i64>,
    working_since: Option<i64>,
}

impl AgentAggregate {
    fn record(&mut self, status: AgentStatus, since: Option<i64>) {
        let (count, since_slot) = match status {
            AgentStatus::Attention => (&mut self.counts.attention, &mut self.attention_since),
            AgentStatus::Waiting => (&mut self.counts.waiting, &mut self.waiting_since),
            AgentStatus::Working => (&mut self.counts.working, &mut self.working_since),
        };
        *count += 1;
        merge_oldest(since_slot, since);
    }

    fn worst(&self) -> Option<(AgentStatus, Option<i64>)> {
        let worst = AgentStatus::ALL
            .into_iter()
            .rev()
            .find(|&status| self.counts.count(status) != 0)?;
        let since = match worst {
            AgentStatus::Working => self.working_since,
            AgentStatus::Waiting => self.waiting_since,
            AgentStatus::Attention => self.attention_since,
        };
        Some((worst, since))
    }
}

fn merge_oldest(slot: &mut Option<i64>, candidate: Option<i64>) {
    match (*slot, candidate) {
        (None, candidate) => *slot = candidate,
        (Some(current), Some(candidate)) if candidate < current => *slot = Some(candidate),
        _ => {}
    }
}

fn apply_agent_status(sessions: &mut [Session], rows: &[PaneAgentRow]) {
    let mut by_session: HashMap<&str, AgentAggregate> = HashMap::new();
    for (session_id, status, since) in rows {
        if let Some(status) = status {
            by_session
                .entry(session_id.as_str())
                .or_default()
                .record(*status, *since);
        }
    }

    for session in sessions.iter_mut() {
        let Some(aggregate) = by_session.get(session.id.as_str()) else {
            continue;
        };
        session.agent_pane_counts = aggregate.counts;
        (session.agent_status, session.agent_status_since) = match aggregate.worst() {
            Some((status, since)) => (Some(status), since),
            None => (None, None),
        };
    }
}

fn parse_sessions_output(output: &str) -> Result<Vec<Session>> {
    parse_rows(output, "list-sessions", |line| {
        let fields = SessionFields::parse(line)?;
        anyhow::ensure!(!fields.id.is_empty(), "missing session id");
        anyhow::ensure!(!fields.name.is_empty(), "missing session name");
        Ok(session_metadata(
            fields.id,
            fields.name,
            fields
                .attached
                .parse::<u32>()
                .context("invalid session_attached")?
                != 0,
            fields
                .window_count
                .parse()
                .context("invalid session_windows")?,
        ))
    })
}

fn session_metadata(id: &str, name: &str, attached: bool, window_count: u32) -> Session {
    Session {
        id: id.to_owned(),
        name: name.to_owned(),
        attached,
        window_count,
        windows: Vec::new(),
        agent_status: None,
        agent_status_since: None,
        agent_pane_counts: AgentPaneCounts::default(),
    }
}

pub(crate) fn tmux_output(args: &[&str], message: &str) -> Result<Output> {
    Command::new("tmux")
        .args(args)
        .output()
        .with_context(|| message.to_owned())
}

pub fn run_tmux(args: &[&str], message: &str) -> Result<String> {
    let output = tmux_output(args, message)?;
    if !output.status.success() {
        return Err(tmux_error(message, &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn tmux_error(message: &str, stderr: &[u8]) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    if stderr.is_empty() {
        anyhow!(message.to_string())
    } else {
        anyhow!("{message}: {stderr}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_session_lines_from_tmux_format() {
        let sessions = parse_sessions(
            "$1\u{1f}dev\u{1f}1\u{1f}4\u{1f}1710000000\u{1f}1710000300\n$2\u{1f}logs\u{1f}0\u{1f}2\u{1f}1710000100\u{1f}1710000400\n",
        );

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, "$1");
        assert_eq!(sessions[0].name, "dev");
        assert!(sessions[0].attached);
        assert_eq!(sessions[0].window_count, 4);
        assert_eq!(sessions[1].name, "logs");
        assert!(!sessions[1].attached);
    }

    #[test]
    fn parses_session_names_containing_pipe_characters() {
        let sessions =
            parse_sessions("$3\u{1f}dev|api\u{1f}0\u{1f}1\u{1f}1710000000\u{1f}1710000300\n");

        assert_eq!(sessions[0].id, "$3");
        assert_eq!(sessions[0].name, "dev|api");
    }

    #[test]
    fn parses_session_names_containing_colons_and_unicode() {
        let sessions = parse_sessions("$3\u{e000}开发:测试\u{e000}0\u{e000}1\n");

        assert_eq!(sessions[0].name, "开发:测试");
        assert_eq!(sessions[0].window_count, 1);
    }

    #[test]
    fn parses_colon_delimited_session_lines() {
        let sessions = parse_sessions("$3:dev|api:0:1:1710000000:1710000300\n");

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "$3");
        assert_eq!(sessions[0].name, "dev|api");
        assert!(!sessions[0].attached);
        assert_eq!(sessions[0].window_count, 1);
    }

    #[test]
    fn rejects_non_empty_unparseable_session_output() {
        let error = parse_sessions_output("$3dev011710000000171000300\n")
            .unwrap_err()
            .to_string();

        assert!(error.contains("unexpected format"));
    }

    #[test]
    fn rejects_partial_session_snapshots() {
        let output = "$1\u{e000}dev\u{e000}0\u{e000}1\ntruncated\n";
        assert!(parse_sessions_output(output).is_err());
    }

    #[test]
    fn rejects_batched_workspace_output_with_missing_window_rows() {
        let sessions = "$1\u{e000}dev\u{e000}0\u{e000}1\n";
        assert!(parse_workspace_snapshot(sessions).is_err());
        let output = format!("{sessions}{WORKSPACE_BOUNDARY}");
        assert!(
            parse_workspace_snapshot(&output)
                .unwrap_err()
                .to_string()
                .contains("topology changed")
        );
    }

    #[test]
    fn parses_pane_agent_rows() {
        let rows = parse_workspace_rows(
            "$1\u{e000}@1\u{e000}0\u{e000}dev\u{e000}80\u{e000}24\u{e000}1\u{e000}%1\u{e000}0\u{e000}0\u{e000}0\u{e000}80\u{e000}24\u{e000}1\u{e000}zsh\u{e000}waiting\u{e000}1700000000\u{e000}\u{e000}0\n",
        ).unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].agent_status, Some(AgentStatus::Waiting));
        assert_eq!(rows[0].agent_status_since, Some(1700000000));
    }

    #[test]
    fn stale_agent_status_does_not_hide_the_pane_or_affect_the_session_badge() {
        let mut sessions = vec![test_session("$1")];
        let rows = parse_workspace_rows(concat!(
            "$1\u{e000}@1\u{e000}0\u{e000}dev\u{e000}80\u{e000}24\u{e000}1\u{e000}",
            "%1\u{e000}0\u{e000}0\u{e000}0\u{e000}80\u{e000}24\u{e000}1\u{e000}",
            "zsh\u{e000}attention\u{e000}100\u{e000}\u{e000}1\n"
        ))
        .unwrap();

        attach_workspace_rows(&mut sessions, &rows);

        assert_eq!(sessions[0].windows[0].panes[0].id, "%1");
        assert_eq!(sessions[0].agent_status, None);
        assert_eq!(sessions[0].agent_status_since, None);
    }

    #[test]
    fn parses_pane_with_no_agent_status_as_none() {
        // Unset tmux pane options render as empty fields, not missing ones.
        let rows = parse_workspace_rows(
            "$1\u{e000}@1\u{e000}0\u{e000}dev\u{e000}80\u{e000}24\u{e000}1\u{e000}%1\u{e000}0\u{e000}0\u{e000}0\u{e000}80\u{e000}24\u{e000}1\u{e000}zsh\u{e000}\u{e000}\u{e000}\u{e000}0\n",
        ).unwrap();

        assert_eq!(rows[0].agent_status, None);
        assert_eq!(rows[0].agent_status_since, None);
    }

    #[test]
    fn ignores_unrecognized_status_values() {
        // Status and timestamp parse independently — an unrecognized status
        // still leaves a well-formed timestamp field intact.
        let rows = parse_workspace_rows(
            "$1\u{e000}@1\u{e000}0\u{e000}dev\u{e000}80\u{e000}24\u{e000}1\u{e000}%1\u{e000}0\u{e000}0\u{e000}0\u{e000}80\u{e000}24\u{e000}1\u{e000}zsh\u{e000}unknown\u{e000}123\u{e000}\u{e000}0\n",
        ).unwrap();

        assert_eq!(rows[0].agent_status, None);
        assert_eq!(rows[0].agent_status_since, Some(123));
    }

    #[test]
    fn session_badge_is_the_worst_pane_status_present() {
        let mut sessions = vec![test_session("$1")];
        let rows = vec![
            ("$1".to_string(), Some(AgentStatus::Working), Some(100)),
            ("$1".to_string(), Some(AgentStatus::Waiting), Some(50)),
            ("$1".to_string(), Some(AgentStatus::Attention), Some(200)),
        ];

        apply_agent_status(&mut sessions, &rows);

        assert_eq!(sessions[0].agent_status, Some(AgentStatus::Attention));
        assert_eq!(sessions[0].agent_status_since, Some(200));
        assert_eq!(
            sessions[0].agent_pane_counts,
            AgentPaneCounts {
                working: 1,
                waiting: 1,
                attention: 1,
            }
        );
    }

    #[test]
    fn badge_timestamp_is_the_oldest_pane_at_the_worst_rank() {
        let mut sessions = vec![test_session("$1")];
        let rows = vec![
            ("$1".to_string(), Some(AgentStatus::Waiting), Some(500)),
            ("$1".to_string(), Some(AgentStatus::Waiting), Some(100)),
        ];

        apply_agent_status(&mut sessions, &rows);

        assert_eq!(sessions[0].agent_status, Some(AgentStatus::Waiting));
        assert_eq!(sessions[0].agent_status_since, Some(100));
    }

    #[test]
    fn sessions_with_no_matching_pane_rows_stay_agent_free() {
        let mut sessions = vec![test_session("$1")];

        apply_agent_status(&mut sessions, &[]);

        assert_eq!(sessions[0].agent_status, None);
        assert_eq!(sessions[0].agent_status_since, None);
    }

    #[test]
    fn groups_workspace_panes_by_session_and_window() {
        let mut sessions = vec![test_session("$1"), test_session("$2")];
        let rows = parse_workspace_rows(concat!(
            "$1\u{e000}@7\u{e000}0\u{e000}编译:测试\u{e000}100\u{e000}30\u{e000}1\u{e000}%3\u{e000}0\u{e000}0\u{e000}0\u{e000}49\u{e000}30\u{e000}1\u{e000}zsh\u{e000}waiting\u{e000}100\u{e000}\u{e000}0\n",
            "$1\u{e000}@7\u{e000}0\u{e000}编译:测试\u{e000}100\u{e000}30\u{e000}1\u{e000}%4\u{e000}1\u{e000}50\u{e000}0\u{e000}50\u{e000}30\u{e000}0\u{e000}cargo\u{e000}\u{e000}\u{e000}\u{e000}0\n",
            "$1\u{e000}@8\u{e000}1\u{e000}日志\u{e000}100\u{e000}30\u{e000}0\u{e000}%5\u{e000}0\u{e000}0\u{e000}0\u{e000}100\u{e000}30\u{e000}1\u{e000}tail\u{e000}\u{e000}\u{e000}\u{e000}0\n",
            "$2\u{e000}@7\u{e000}0\u{e000}编译:测试\u{e000}100\u{e000}30\u{e000}1\u{e000}%3\u{e000}0\u{e000}0\u{e000}0\u{e000}49\u{e000}30\u{e000}1\u{e000}zsh\u{e000}waiting\u{e000}100\u{e000}\u{e000}0\n",
        )).unwrap();
        attach_workspace_rows(&mut sessions, &rows);

        assert_eq!(sessions[0].windows.len(), 2);
        assert_eq!(sessions[0].windows[0].name, "编译:测试");
        assert_eq!(sessions[0].windows[0].panes.len(), 2);
        assert_eq!(sessions[0].windows[0].panes[1].left, 50);
        assert_eq!(sessions[0].windows[1].panes[0].id, "%5");
        assert_eq!(sessions[1].windows.len(), 1);
        assert_eq!(sessions[1].windows[0].panes[0].id, "%3");
        assert_eq!(sessions[0].agent_status, Some(AgentStatus::Waiting));
        assert_eq!(sessions[1].agent_status, Some(AgentStatus::Waiting));
    }

    #[test]
    fn restores_the_unzoomed_geometry_of_the_active_pane() {
        let rows = parse_workspace_rows(concat!(
            "$1\u{e000}@1\u{e000}0\u{e000}dev\u{e000}100\u{e000}24\u{e000}1\u{e000}%1\u{e000}1\u{e000}",
            "0\u{e000}0\u{e000}100\u{e000}24\u{e000}1\u{e000}zsh\u{e000}\u{e000}\u{e000}",
            "c522,120x30,0,0{60x30,0,0,0,59x30,61,0[59x15,61,0,1,59x14,61,16,2]}\u{e000}0"
        )).unwrap();
        let [row] = rows.as_slice() else {
            panic!("missing zoomed pane")
        };
        assert_eq!((row.window.width, row.window.height), (120, 30));
        assert_eq!((row.pane.left, row.pane.top), (61, 0));
        assert_eq!((row.pane.width, row.pane.height), (59, 15));
        assert!(row.pane.active);
    }

    #[test]
    fn rejects_workspace_rows_without_session_window_or_pane_id() {
        let row = "$1\u{e000}@7\u{e000}0\u{e000}dev\u{e000}80\u{e000}24\u{e000}1\u{e000}%3\u{e000}0\u{e000}0\u{e000}0\u{e000}80\u{e000}24\u{e000}1\u{e000}zsh\u{e000}\u{e000}\u{e000}\u{e000}0";
        assert_eq!(parse_workspace_rows(row).unwrap().len(), 1);
        assert!(parse_workspace_rows(&row.replace(WORKSPACE_SEPARATOR, "\u{1f}")).is_err());
        assert!(parse_workspace_row(&row.replacen("$1", "", 1)).is_err());
        assert!(parse_workspace_row(&row.replacen("@7", "", 1)).is_err());
        assert!(parse_workspace_row(&row.replacen("%3", "", 1)).is_err());
    }

    fn test_session(id: &str) -> Session {
        Session {
            id: id.to_string(),
            name: id.to_string(),
            attached: false,
            window_count: 1,
            windows: Vec::new(),
            agent_status: None,
            agent_status_since: None,
            agent_pane_counts: AgentPaneCounts::default(),
        }
    }
}
