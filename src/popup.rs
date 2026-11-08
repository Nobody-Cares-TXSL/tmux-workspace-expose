use std::{
    env, fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use tmux_expose::{model::App, tmux::run_tmux};

const RESIZE_CONFIRM_DELAY: Duration = Duration::from_millis(220);
const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// tmux 3.4 caps an open popup at its original dimensions. Return to its
/// launcher once the client settles so a fresh popup uses the new dimensions.
pub struct Resize {
    client: String,
    state: PathBuf,
    opened_at_size: String,
    candidate: Option<String>,
    next_check: Instant,
}

impl Resize {
    pub fn from_env(client: Option<&str>, app: &mut App) -> Option<Self> {
        let client = client?.to_owned();
        let state = PathBuf::from(env::var_os("TMUX_EXPOSE_RESUME")?);
        let opened_at_size = env::var("TMUX_EXPOSE_OPEN_SIZE").ok()?;
        let saved = fs::read_to_string(&state).ok()?;
        if saved.is_empty() {
            match env::var("TMUX_EXPOSE_START").as_deref() {
                Ok("1") => app.open_grid_settings(),
                Ok("2") => app.open_current_windows(),
                _ => {}
            }
        } else {
            app.restore_popup_state(&saved);
            fs::write(&state, "").ok()?;
        }
        Some(Self {
            client,
            state,
            opened_at_size,
            candidate: None,
            next_check: Instant::now() + RESIZE_CONFIRM_DELAY,
        })
    }

    pub fn on_resize(&mut self) {
        if self.candidate.is_none() {
            self.next_check = Instant::now();
        }
    }

    pub fn reopen(&mut self, app: &App) -> bool {
        let now = Instant::now();
        if now < self.next_check {
            return false;
        }
        let output = run_tmux(
            &[
                "display-message",
                "-p",
                "-c",
                &self.client,
                "#{client_width} #{client_height}",
            ],
            "Could not read tmux client dimensions",
        );
        let size = output.as_deref().unwrap_or_default().trim();
        self.observe_size(size, Instant::now()) && fs::write(&self.state, app.popup_state()).is_ok()
    }

    fn observe_size(&mut self, size: &str, now: Instant) -> bool {
        if size.is_empty() || size == self.opened_at_size {
            self.candidate = None;
            self.next_check = now + IDLE_CHECK_INTERVAL;
            return false;
        }
        self.next_check = now + RESIZE_CONFIRM_DELAY;
        if self.candidate.as_deref() != Some(size) {
            self.candidate = Some(size.to_owned());
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slows_idle_checks_but_requires_two_matching_resize_samples() {
        let now = Instant::now();
        let mut resize = Resize {
            client: String::new(),
            state: PathBuf::new(),
            opened_at_size: "80 24".into(),
            candidate: None,
            next_check: now,
        };
        assert!(!resize.observe_size("80 24", now));
        assert_eq!(resize.next_check, now + IDLE_CHECK_INTERVAL);
        assert!(!resize.observe_size("100 28", now));
        assert_eq!(resize.next_check, now + RESIZE_CONFIRM_DELAY);
        resize.on_resize();
        assert_eq!(resize.next_check, now + RESIZE_CONFIRM_DELAY);
        assert!(!resize.observe_size("101 28", now));
        assert!(resize.observe_size("101 28", now + RESIZE_CONFIRM_DELAY));
        assert!(!resize.observe_size("", now));
        assert!(!resize.observe_size("101 28", now));
    }
}
