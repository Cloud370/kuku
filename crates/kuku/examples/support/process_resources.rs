#[cfg(target_os = "linux")]
pub(super) fn sample() -> Option<serde_json::Value> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes the caller-owned rusage on success.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return None;
    }
    let usage = unsafe { usage.assume_init() };
    let seconds = |time: libc::timeval| time.tv_sec as f64 + time.tv_usec as f64 / 1_000_000.0;
    Some(serde_json::json!({
        "user_cpu_seconds": seconds(usage.ru_utime),
        "system_cpu_seconds": seconds(usage.ru_stime),
        "peak_rss_kib": usage.ru_maxrss,
        "input_blocks": usage.ru_inblock,
        "output_blocks": usage.ru_oublock,
    }))
}

#[cfg(not(target_os = "linux"))]
pub(super) fn sample() -> Option<serde_json::Value> {
    None
}
