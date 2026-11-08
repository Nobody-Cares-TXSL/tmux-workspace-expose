use std::{env, io, path::PathBuf};

pub(crate) fn path(file: &str) -> io::Result<PathBuf> {
    let directory = env::var_os("XDG_CONFIG_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "No configuration directory"))?;
    Ok(directory.join("tmux-expose").join(file))
}
