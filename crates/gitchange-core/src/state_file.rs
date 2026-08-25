//! State-file persistence (ADR 0002): one pretty-printed JSON file at
//! `$GIT_DIR/gitchange/state.json`, atomic write-then-rename, fail-fast
//! lockfile, schema version field.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use crate::error::{Error, LockHolder};
use crate::state::{SCHEMA_VERSION, State};

/// Declares the state file's name and its lock/tmp siblings from one
/// literal (a `macro_rules!` because `concat!` needs a literal token, not
/// a `const` reference): renaming the file can't leave a sibling on the
/// old stem.
macro_rules! state_file_names {
    ($stem:literal) => {
        pub(crate) const STATE_FILE: &str = $stem;
        const LOCK_FILE: &str = concat!($stem, ".lock");
        const TMP_FILE: &str = concat!($stem, ".tmp");
    };
}
state_file_names!("state.json");

/// A held lockfile; dropping it releases the lock. Writers must hold one
/// across their load-mutate-save cycle.
#[derive(Debug)]
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

/// Take the lockfile, failing fast if another process holds it. The
/// writer's PID goes in the file, so the next writer to find it taken can
/// tell a live holder from a leaked lock ([`LockHolder`]).
pub(crate) fn lock(dir: &Path) -> Result<Lock, Error> {
    fs::create_dir_all(dir).map_err(state_error)?;
    let path = dir.join(LOCK_FILE);
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            // The guard exists before the write, so a failure here
            // releases the lock we just took rather than leaking it.
            let lock = Lock { path };
            writeln!(file, "{}", std::process::id()).map_err(state_error)?;
            Ok(lock)
        }
        Err(err) if err.kind() == ErrorKind::AlreadyExists => Err(Error::LockContention {
            holder: holder_of(&path),
            path,
        }),
        Err(err) => Err(state_error(err)),
    }
}

/// Classify the holder of an already-taken lockfile.
fn holder_of(path: &Path) -> LockHolder {
    let pid = fs::read_to_string(path)
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok());
    // PID 0 addresses no process (and on unix would mean the caller's own
    // process group), so it is unreadable rather than dead.
    match pid {
        None | Some(0) => LockHolder::Unreadable,
        Some(pid) if process_is_running(pid) => LockHolder::Alive { pid },
        Some(pid) => LockHolder::Dead { pid },
    }
}

/// Whether `pid` names a running process. Every answer the OS refuses to
/// give resolves to `true`: a lock is called stale only on proof, never on
/// a guess.
#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return true;
    };
    // Signal 0 runs kill's existence and permission checks without
    // delivering anything. Only ESRCH — no such process — means gone;
    // EPERM is a live process this user does not own.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(windows)]
fn process_is_running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // A handle that cannot be opened is only proof of death for
    // ERROR_INVALID_PARAMETER, which is what a nonexistent PID gives;
    // access denied and the rest mean a process we merely cannot inspect.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false.into(), pid) };
    if handle.is_null() {
        return std::io::Error::last_os_error().raw_os_error()
            != Some(ERROR_INVALID_PARAMETER as i32);
    }

    let mut code = 0u32;
    let read = unsafe { GetExitCodeProcess(handle, &mut code) };
    unsafe { CloseHandle(handle) };
    // A handle can outlive its process, so still-running is the exit code
    // saying so — and an unreadable code is another guess, hence alive.
    read == 0 || code as i32 == STILL_ACTIVE
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The one lockfile property core's integration seam cannot see: the
    /// recorded PID exists only while the lock is held, and every public op
    /// releases before it returns. Contention resolution — what callers do
    /// see — is asserted at that seam (`tests/core/lockfile.rs`).
    #[test]
    fn taking_the_lock_records_the_writers_pid() {
        let dir = tempfile::tempdir().unwrap();
        let held = lock(dir.path()).unwrap();

        let recorded = fs::read_to_string(dir.path().join(LOCK_FILE)).unwrap();
        assert_eq!(recorded.trim().parse::<u32>(), Ok(std::process::id()));

        // And it reads back as the live holder it is — this process.
        let err = lock(dir.path()).unwrap_err();
        assert!(
            matches!(err, Error::LockContention { holder: LockHolder::Alive { pid }, .. }
                if pid == std::process::id()),
            "{err:?}"
        );

        drop(held);
        assert!(!dir.path().join(LOCK_FILE).exists());
    }
}
