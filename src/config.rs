use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use anyhow::{Context, Result};

pub(crate) fn path(file: &str) -> io::Result<PathBuf> {
    let directory = env::var_os("XDG_CONFIG_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "No configuration directory"))?;
    Ok(directory.join("tmux-expose").join(file))
}

pub(crate) fn save<T>(
    files: &[(&str, String)],
    prepare: impl FnOnce() -> Result<T>,
    apply: impl FnOnce(T) -> Result<()>,
) -> Result<()> {
    let config = path("")?;
    save_in(&config, files, prepare, apply)
}

fn save_in<T>(
    directory: &Path,
    files: &[(&str, String)],
    prepare: impl FnOnce() -> Result<T>,
    apply: impl FnOnce(T) -> Result<()>,
) -> Result<()> {
    if files.is_empty() {
        return apply(prepare()?);
    }
    fs::create_dir_all(directory)?;
    let lock = File::open(directory)?;
    lock.lock()?;
    let plan = prepare()?;

    let mut changes = Vec::with_capacity(files.len());
    for (name, data) in files {
        let path = directory.join(name);
        let (path, before) = match fs::canonicalize(&path) {
            Ok(path) => {
                let before = fs::read(&path).with_context(|| format!("Read {}", path.display()))?;
                (path, Some(before))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => (path, None),
            Err(error) => return Err(error).with_context(|| format!("Read {}", path.display())),
        };
        changes.push((path, before, data));
    }

    let mut written = 0;
    let result = (|| {
        for (path, _, data) in &changes {
            replace(path, data.as_bytes()).with_context(|| format!("Save {}", path.display()))?;
            written += 1;
        }
        apply(plan)
    })();
    if let Err(mut error) = result {
        for (path, before, _) in changes[..written].iter().rev() {
            let restore = match before {
                Some(data) => replace(path, data),
                None => fs::remove_file(path),
            };
            if let Err(restore) = restore {
                error = anyhow::anyhow!("{error:#}\nRestore {}: {restore}", path.display());
            }
        }
        return Err(error);
    }
    Ok(())
}

fn replace(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
    let mode = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let (temp, mut file) = loop {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let temp = path.with_file_name(format!(".tmux-expose-{}-{sequence}", std::process::id()));
        match OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temp)
        {
            Ok(file) => break (temp, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        file.write_all(data)?;
        if let Some(mode) = mode {
            file.set_permissions(mode)?;
        }
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_exact_contents_and_absence_when_apply_fails() {
        let directory =
            env::temp_dir().join(format!("tmux-expose-save-test-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let grid = directory.join("grid.conf");
        let appearance = directory.join("appearance.conf");
        fs::write(&grid, b" 2 2 2 2 \n").unwrap();

        let files = [
            ("grid.conf", "3 3 3 3 1\n".into()),
            ("appearance.conf", "language=en\n".into()),
        ];
        let result = save_in(
            &directory,
            &files,
            || Ok(()),
            |()| {
                assert_eq!(fs::read_to_string(&grid)?, "3 3 3 3 1\n");
                assert_eq!(fs::read_to_string(&appearance)?, "language=en\n");
                anyhow::bail!("binding reload failed")
            },
        );

        assert_eq!(result.unwrap_err().to_string(), "binding reload failed");
        assert_eq!(fs::read(&grid).unwrap(), b" 2 2 2 2 \n");
        assert!(!appearance.exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
