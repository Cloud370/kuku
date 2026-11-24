#[cfg(target_os = "linux")]
use std::collections::BTreeMap;

#[cfg(target_os = "linux")]
pub(super) fn sample() -> Option<serde_json::Value> {
    let proc_io = std::fs::read_to_string("/proc/self/io").ok();
    let io = proc_io.as_deref().and_then(parse_io);
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes the caller-owned rusage on success.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return io.map(|io| serde_json::json!({"process_io": io}));
    }
    // SAFETY: The successful getrusage call initialized the local buffer.
    let usage = unsafe { usage.assume_init() };
    let status = std::fs::read_to_string("/proc/self/status").ok();
    let peak_rss = status
        .as_deref()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("VmHWM:")?
                    .split_whitespace()
                    .next()?
                    .parse()
                    .ok()
            })
        })
        .unwrap_or(usage.ru_maxrss);
    let seconds = |time: libc::timeval| time.tv_sec as f64 + time.tv_usec as f64 / 1_000_000.0;
    Some(serde_json::json!({
        "user_cpu_seconds": seconds(usage.ru_utime),
        "system_cpu_seconds": seconds(usage.ru_stime),
        "peak_rss_kib": peak_rss,
        "input_blocks": usage.ru_inblock,
        "output_blocks": usage.ru_oublock,
        "process_io": io,
    }))
}

#[cfg(target_os = "linux")]
fn parse_io(contents: &str) -> Option<BTreeMap<&str, u64>> {
    const FIELDS: [&str; 7] = [
        "rchar",
        "wchar",
        "syscr",
        "syscw",
        "read_bytes",
        "write_bytes",
        "cancelled_write_bytes",
    ];
    let mut observations = BTreeMap::new();
    for line in contents.lines() {
        let Some((field, value)) = line.split_once(':') else {
            continue;
        };
        if FIELDS.contains(&field) {
            observations.insert(field, value.trim().parse().ok()?);
        }
    }
    (observations.len() == FIELDS.len()).then_some(observations)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn sample() -> Option<serde_json::Value> {
    None
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::collections::BTreeMap;

    use super::parse_io;

    #[test]
    fn proc_io_requires_all_seven_counters() {
        let complete = "rchar: 41\nwchar: 8\nsyscr: 3\nsyscw: 1\nread_bytes: 0\nwrite_bytes: 4096\ncancelled_write_bytes: 0\n";
        assert_eq!(
            Some(BTreeMap::from([
                ("rchar", 41),
                ("wchar", 8),
                ("syscr", 3),
                ("syscw", 1),
                ("read_bytes", 0),
                ("write_bytes", 4096),
                ("cancelled_write_bytes", 0),
            ])),
            parse_io(complete)
        );
        assert_eq!(None, parse_io(complete.lines().next().unwrap()));
        assert_eq!(None, parse_io(&complete.replace("wchar: 8", "wchar: -1")));
    }
}
