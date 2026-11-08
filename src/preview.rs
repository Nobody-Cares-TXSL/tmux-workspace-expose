use std::{
    collections::{HashMap, HashSet},
    thread,
    time::{Duration, Instant},
};

use crate::{model::Session, tmux::capture_pane_preview};

#[derive(Clone, Copy)]
pub struct RefreshPolicy {
    pub selected_pane: Duration,
    pub selected_session: Duration,
    pub background: Duration,
    pub max_captures: usize,
}

impl Default for RefreshPolicy {
    fn default() -> Self {
        Self {
            selected_pane: Duration::from_millis(300),
            selected_session: Duration::from_millis(700),
            background: Duration::from_millis(1500),
            max_captures: 4,
        }
    }
}

pub struct CachedPreview {
    pub lines: Vec<String>,
    pub failed: bool,
    attempted_at: Instant,
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
    ) {
        let due = self.due_panes(
            visible,
            selected_session,
            selected_pane,
            Instant::now(),
            policy,
        );
        thread::scope(|scope| {
            let jobs: Vec<_> = due
                .into_iter()
                .map(|id| (id, scope.spawn(move || capture_pane_preview(id))))
                .collect();
            for (id, job) in jobs {
                let result = job
                    .join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("capture worker failed")));
                self.record(id.to_owned(), Instant::now(), result);
            }
        });
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

    fn record(&mut self, id: String, now: Instant, result: anyhow::Result<Vec<String>>) {
        let (lines, failed) = match result {
            Ok(lines) => (lines, false),
            Err(_) => {
                let previous = self.by_pane.remove(&id);
                (
                    previous.map_or_else(Vec::new, |preview| preview.lines),
                    true,
                )
            }
        };
        self.by_pane.insert(
            id,
            CachedPreview {
                lines,
                failed,
                attempted_at: now,
            },
        );
    }
}

fn pane_ids(session: &Session) -> impl Iterator<Item = &str> {
    session
        .windows
        .iter()
        .flat_map(|window| window.panes.iter().map(|pane| pane.id.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PanePreview, WindowPreview};
    use crate::tmux::parse_sessions;

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
