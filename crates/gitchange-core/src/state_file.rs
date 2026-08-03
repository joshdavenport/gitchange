//! State-file persistence (ADR 0002): one pretty-printed JSON file at
//! `$GIT_DIR/gitchange/state.json`, atomic write-then-rename, fail-fast
//! lockfile, schema version field.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use crate::error::Error;
use crate::state::{SCHEMA_VERSION, State};

/// Declares the state file's name and its lock/tmp siblings from one
/// literal (a `macro_rules!` because `concat!` needs a literal token, not
/// a `const` reference): renaming the file can't leave a sibling on the
/// old stem.
macro_rules! state_file_names {
    ($stem:literal) => {
        const STATE_FILE: &str = $stem;
        const LOCK_FILE: &str = concat!($stem, ".lock");
        const TMP_FILE: &str = concat!($stem, ".tmp");
    };
}
state_file_names!("state.json");

/// A held lockfile; dropping it releases the lock. Writers must hold one
/// across their load-mutate-save cycle.
pub(crate) struct Lock {
    path: PathBuf,
}

impl Drop for Lock {
    fn drop(&mut self) {
        // Best-effort: a leaked lockfile is reported to the next writer
        // by LockContention's message, never silently stolen.
        let _ = fs::remove_file(&self.path);
    }
}

/// Take the lockfile, failing fast if another process holds it.
pub(crate) fn lock(dir: &Path) -> Result<Lock, Error> {
    fs::create_dir_all(dir).map_err(state_error)?;
    let path = dir.join(LOCK_FILE);
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(_) => Ok(Lock { path }),
        Err(err) if err.kind() == ErrorKind::AlreadyExists => Err(Error::LockContention { path }),
        Err(err) => Err(state_error(err)),
    }
}

/// Read the state file; a missing file is an empty state, not an error.
pub(crate) fn load(dir: &Path) -> Result<State, Error> {
    let raw = match fs::read_to_string(dir.join(STATE_FILE)) {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Ok(State::default());
        }
        Err(err) => return Err(state_error(err)),
    };
    let state: State = serde_json::from_str(&raw).map_err(state_error)?;
    if state.version != SCHEMA_VERSION {
        return Err(Error::State(
            format!(
                "unsupported state schema version {} (this gitchange reads version {})",
                state.version, SCHEMA_VERSION
            )
            .into(),
        ));
    }
    Ok(state)
}

/// Atomically replace the state file (write-then-rename). The caller
/// holds the [`Lock`].
pub(crate) fn save(dir: &Path, state: &State) -> Result<(), Error> {
    let mut json = serde_json::to_string_pretty(state).map_err(state_error)?;
    json.push('\n');
    let tmp = dir.join(TMP_FILE);
    {
        let mut file = fs::File::create(&tmp).map_err(state_error)?;
        file.write_all(json.as_bytes()).map_err(state_error)?;
        // Flushed before the rename so a crash can't publish a
        // truncated state file under the final name.
        file.sync_all().map_err(state_error)?;
    }
    fs::rename(&tmp, dir.join(STATE_FILE)).map_err(state_error)
}

fn state_error(err: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::State(Box::new(err))
}
