//! Peak-RSS probe for the huge-file memory cases. High-water mark for
//! the whole process — which is why every case runs in its own
//! subprocess: earlier cases can't inflate a later reading.

#[cfg(unix)]
pub fn peak_rss_bytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    let usage = unsafe { usage.assume_init() };
    let raw = u64::try_from(usage.ru_maxrss).ok()?;
    // macOS reports ru_maxrss in bytes, other unixes in kilobytes.
    Some(if cfg!(target_os = "macos") {
        raw
    } else {
        raw * 1024
    })
}

#[cfg(not(unix))]
pub fn peak_rss_bytes() -> Option<u64> {
    // Windows would need GetProcessMemoryInfo; the memory probe is a
    // dev-machine measurement (issue #29 — CI runs are a desirable
    // extra), so absent rather than wrong.
    None
}
