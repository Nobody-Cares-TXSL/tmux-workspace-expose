use std::{
    env, fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use tmux_expose::{model::App, tmux::run_tmux};

/// tmux 3.4 caps an open popup at its original dimensions. Return to its
/// launcher once the client settles so a fresh popup uses the new dimensions.
pub struct Resize {
    client: String,
    state: PathBuf,
    opened_at_size: String,
    candidate: Option<String>,
    checked_at: Instant,
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
            checked_at: Instant::now(),
        })
    }

    pub fn reopen(&mut self, app: &App) -> bool {
        if self.checked_at.elapsed() < Duration::from_millis(220) {
            return false;
        }
        self.checked_at = Instant::now();
        let Ok(output) = run_tmux(
            &[
                "display-message",
                "-p",
                "-c",
                &self.client,
                "#{client_width} #{client_height}",
            ],
            "Could not read tmux client dimensions",
        ) else {
            return false;
        };
        let size = output.trim().to_owned();
        if size.is_empty() || size == self.opened_at_size {
            self.candidate = None;
            return false;
        }
        if self.candidate.as_ref() != Some(&size) {
            self.candidate = Some(size);
            return false;
        }
        fs::write(&self.state, app.popup_state()).is_ok()
    }
}
