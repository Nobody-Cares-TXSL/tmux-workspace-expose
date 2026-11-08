use std::collections::HashMap;
use std::process::Command;

use anyhow::{Context, Result, anyhow};

use crate::model::{AgentPaneCounts, AgentStatus, PanePreview, Session, WindowPreview};

const FIELD_SEPARATOR: char = ':';
const LEGACY_FIELD_SEPARATOR: char = '\u{1f}';
// tmux escapes control characters in -F output (US becomes literal "\\037").
// A printable private-use character keeps fields separate without colliding
// with ordinary punctuation in session and window names.
const WORKSPACE_SEPARATOR: char = '\u{e000}';
const SESSION_FORMAT: &str =
    "#{session_id}\u{e000}#{session_name}\u{e000}#{session_attached}\u{e000}#{session_windows}";
const WORKSPACE_PANE_FORMAT: &str = concat!(
    "#{session_id}\u{e000}#{window_id}\u{e000}#{window_index}\u{e000}#{window_name}\u{e000}",
    "#{window_width}\u{e000}#{window_height}\u{e000}#{window_active}\u{e000}",
    "#{pane_id}\u{e000}#{pane_index}\u{e000}#{pane_left}\u{e000}#{pane_top}\u{e000}",
    "#{pane_width}\u{e000}#{pane_height}\u{e000}#{pane_active}\u{e000}",
    "#{pane_current_command}\u{e000}#{@agent_status}\u{e000}#{@agent_status_since}\u{e000}",
    "#{?window_zoomed_flag,#{window_layout},}"
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

fn parse_workspace_rows(output: &str) -> Vec<WorkspaceRow> {
    output.lines().filter_map(parse_workspace_row).collect()
}

fn parse_workspace_row(line: &str) -> Option<WorkspaceRow> {
    let separator = if line.contains(WORKSPACE_SEPARATOR) {
        WORKSPACE_SEPARATOR
    } else {
        LEGACY_FIELD_SEPARATOR
    };
    let fields: Vec<&str> = line.split(separator).collect();
    if !(17..=18).contains(&fields.len()) || fields[0].is_empty() {
        return None;
    }
    let mut row = WorkspaceRow {
        session_id: fields[0].to_string(),
        window: parse_window_fields(&fields)?,
        pane: parse_pane_fields(&fields)?,
        agent_status: AgentStatus::parse(fields[15]),
        agent_status_since: fields[16].parse().ok(),
    };
    if let Some(&layout) = fields.get(17).filter(|layout| !layout.is_empty()) {
        restore_unzoomed_geometry(layout, &mut row.window, &mut row.pane)?;
    }
    Some(row)
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

fn parse_window_fields(fields: &[&str]) -> Option<WindowPreview> {
    Some(WindowPreview {
        id: (!fields[1].is_empty()).then(|| fields[1].to_string())?,
        index: fields[2].parse().ok()?,
        name: fields[3].to_string(),
        width: fields[4].parse().ok()?,
        height: fields[5].parse().ok()?,
        active: fields[6] == "1",
        panes: Vec::new(),
    })
}

fn parse_pane_fields(fields: &[&str]) -> Option<PanePreview> {
    Some(PanePreview {
        id: (!fields[7].is_empty()).then(|| fields[7].to_string())?,
        index: fields[8].parse().ok()?,
        left: fields[9].parse().ok()?,
        top: fields[10].parse().ok()?,
        width: fields[11].parse().ok()?,
        height: fields[12].parse().ok()?,
        active: fields[13] == "1",
        command: fields[14].to_string(),
    })
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
    let mut sessions = list_session_metadata()?;
    let rows = list_workspace_rows()?;
    attach_workspace_rows(&mut sessions, &rows);
    if sessions
        .iter()
        .any(|session| session.windows.len() != session.window_count as usize)
    {
        return Err(anyhow!("tmux workspace topology changed during refresh"));
    }
    Ok(sessions)
}

fn list_session_metadata() -> Result<Vec<Session>> {
    let output = Command::new("tmux")
        .args(["list-sessions", "-F", SESSION_FORMAT])
        .output()
        .context("failed to run tmux list-sessions")?;

    if !output.status.success() {
        return Err(tmux_error("tmux list-sessions failed", &output.stderr));
    }

    parse_sessions_output(&String::from_utf8_lossy(&output.stdout))
}

pub fn current_session_id() -> Result<Option<String>> {
    let output = Command::new("tmux")
        .args(["display-message", "-p", "#{session_id}"])
        .output()
        .context("failed to run tmux display-message")?;

    if !output.status.success() {
        return Ok(None);
    }

    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!id.is_empty()).then_some(id))
}

pub fn capture_pane_preview(pane_id: &str) -> Result<Vec<String>> {
    Ok(capture_screen(pane_id)?
        .lines()
        .map(str::to_owned)
        .collect())
}

fn capture_screen(target: &str) -> Result<String> {
    let output = Command::new("tmux")
        .args(["capture-pane", "-e", "-p", "-t", target])
        .output()
        .with_context(|| format!("failed to capture tmux pane '{target}'"))?;

    if !output.status.success() {
        return Err(tmux_error("tmux capture-pane failed", &output.stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
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
    let mut tmux = Command::new("tmux");
    tmux.args([command, "-t", id]);
    if let Some(name) = name {
        if name.contains(WORKSPACE_SEPARATOR) {
            return Err(anyhow!("Name contains an unsupported character"));
        }
        // tmux expands formats in names and treats a final semicolon as a command separator.
        let mut literal = name.replace('#', "##");
        if literal.ends_with(';') {
            literal.insert(literal.len() - 1, '\\');
        }
        tmux.args(["--", &literal]);
    }
    let output = tmux
        .output()
        .with_context(|| format!("{command} failed for {}", target.description()))?;
    if !output.status.success() {
        return Err(tmux_error(
            &format!("{command} failed for {}", target.description()),
            &output.stderr,
        ));
    }
    Ok(())
}

/// Like `switch_client`, but for a specific client. Commands run from a key
/// binding via `run-shell` aren't attached to any client, so they have to
/// name the one that pressed the key or tmux guesses.
pub fn switch_client_for(client: Option<&str>, session_target: &str) -> Result<()> {
    let mut args = vec!["switch-client"];
    if let Some(client) = client {
        args.extend(["-c", client]);
    }
    args.extend(["-t", session_target]);

    let output = Command::new("tmux")
        .args(&args)
        .output()
        .with_context(|| format!("failed to switch to tmux session '{session_target}'"))?;

    if !output.status.success() {
        return Err(tmux_error(
            &format!("tmux switch-client failed for '{session_target}'"),
            &output.stderr,
        ));
    }

    Ok(())
}

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
            let id = parts.next()?.to_string();
            let name = parts.next()?.to_string();
            if name.is_empty() {
                return None;
            }

            let attached = parts.next().is_some_and(|value| value != "0");
            let window_count = parts
                .next()
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(0);
            Some(Session {
                id,
                name,
                attached,
                window_count,
                windows: Vec::new(),
                agent_status: None,
                agent_status_since: None,
                agent_pane_counts: AgentPaneCounts::default(),
            })
        })
        .collect()
}

/// One tmux query supplies the workspace tree and its agent status badges.
fn list_workspace_rows() -> Result<Vec<WorkspaceRow>> {
    let stdout = list_pane_output(WORKSPACE_PANE_FORMAT)?;
    let rows = parse_workspace_rows(&stdout);
    if rows.len() != stdout.lines().count() {
        return Err(anyhow!("unexpected format from tmux list-panes"));
    }
    Ok(rows)
}

fn list_pane_output(format: &str) -> Result<String> {
    let output = Command::new("tmux")
        .args(["list-panes", "-a", "-F", format])
        .output()
        .context("failed to run tmux list-panes")?;
    if !output.status.success() {
        return Err(tmux_error("tmux list-panes failed", &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
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
        if self.counts.attention > 0 {
            Some((AgentStatus::Attention, self.attention_since))
        } else if self.counts.waiting > 0 {
            Some((AgentStatus::Waiting, self.waiting_since))
        } else if self.counts.working > 0 {
            Some((AgentStatus::Working, self.working_since))
        } else {
            None
        }
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
    let sessions = parse_sessions(output);
    if sessions.is_empty() && !output.trim().is_empty() {
        return Err(anyhow!(
            "tmux list-sessions returned output in an unexpected format"
        ));
    }

    Ok(sessions)
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
    fn parses_pane_agent_rows() {
        let rows = parse_workspace_rows(
            "$1\u{1f}@1\u{1f}0\u{1f}dev\u{1f}80\u{1f}24\u{1f}1\u{1f}%1\u{1f}0\u{1f}0\u{1f}0\u{1f}80\u{1f}24\u{1f}1\u{1f}zsh\u{1f}waiting\u{1f}1700000000\n",
        );

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].agent_status, Some(AgentStatus::Waiting));
        assert_eq!(rows[0].agent_status_since, Some(1700000000));
    }

    #[test]
    fn parses_pane_with_no_agent_status_as_none() {
        // Unset tmux pane options render as empty fields, not missing ones.
        let rows = parse_workspace_rows(
            "$1\u{1f}@1\u{1f}0\u{1f}dev\u{1f}80\u{1f}24\u{1f}1\u{1f}%1\u{1f}0\u{1f}0\u{1f}0\u{1f}80\u{1f}24\u{1f}1\u{1f}zsh\u{1f}\u{1f}\n",
        );

        assert_eq!(rows[0].agent_status, None);
        assert_eq!(rows[0].agent_status_since, None);
    }

    #[test]
    fn ignores_unrecognized_status_values() {
        // Status and timestamp parse independently — an unrecognized status
        // still leaves a well-formed timestamp field intact.
        let rows = parse_workspace_rows(
            "$1\u{1f}@1\u{1f}0\u{1f}dev\u{1f}80\u{1f}24\u{1f}1\u{1f}%1\u{1f}0\u{1f}0\u{1f}0\u{1f}80\u{1f}24\u{1f}1\u{1f}zsh\u{1f}unknown\u{1f}123\n",
        );

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
            "$1\u{1f}@7\u{1f}0\u{1f}编译:测试\u{1f}100\u{1f}30\u{1f}1\u{1f}%3\u{1f}0\u{1f}0\u{1f}0\u{1f}49\u{1f}30\u{1f}1\u{1f}zsh\u{1f}waiting\u{1f}100\n",
            "$1\u{1f}@7\u{1f}0\u{1f}编译:测试\u{1f}100\u{1f}30\u{1f}1\u{1f}%4\u{1f}1\u{1f}50\u{1f}0\u{1f}50\u{1f}30\u{1f}0\u{1f}cargo\u{1f}\u{1f}\n",
            "$1\u{1f}@8\u{1f}1\u{1f}日志\u{1f}100\u{1f}30\u{1f}0\u{1f}%5\u{1f}0\u{1f}0\u{1f}0\u{1f}100\u{1f}30\u{1f}1\u{1f}tail\u{1f}\u{1f}\n",
            "$2\u{1f}@7\u{1f}0\u{1f}编译:测试\u{1f}100\u{1f}30\u{1f}1\u{1f}%3\u{1f}0\u{1f}0\u{1f}0\u{1f}49\u{1f}30\u{1f}1\u{1f}zsh\u{1f}waiting\u{1f}100\n",
        ));
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
            "c522,120x30,0,0{60x30,0,0,0,59x30,61,0[59x15,61,0,1,59x14,61,16,2]}"
        ));
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
        let row = "$1\u{1f}@7\u{1f}0\u{1f}dev\u{1f}80\u{1f}24\u{1f}1\u{1f}%3\u{1f}0\u{1f}0\u{1f}0\u{1f}80\u{1f}24\u{1f}1\u{1f}zsh\u{1f}\u{1f}";
        assert_eq!(parse_workspace_rows(row).len(), 1);
        assert_eq!(
            parse_workspace_rows(&row.replace(LEGACY_FIELD_SEPARATOR, "\u{e000}")).len(),
            1
        );
        assert!(parse_workspace_row(&row.replacen("$1", "", 1)).is_none());
        assert!(parse_workspace_row(&row.replacen("@7", "", 1)).is_none());
        assert!(parse_workspace_row(&row.replacen("%3", "", 1)).is_none());
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
