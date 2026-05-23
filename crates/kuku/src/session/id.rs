use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::Result;

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a new unique session ID: `YYYYMMDD-HHmm-xxxx`.
pub fn new_session_id() -> String {
    let now = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
    let date = now.date();
    let time = now.time();
    let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut rng: u16 = (counter
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407)) as u16;
    rng ^= std::process::id() as u16;
    rng ^= (counter.wrapping_mul(17)) as u16;
    format!(
        "{:04}{:02}{:02}-{:02}{:02}-{:04x}",
        date.year(),
        u8::from(date.month()),
        date.day(),
        time.hour(),
        time.minute(),
        rng
    )
}

/// Validate a session ID against path-traversal and reserved-name rules.
pub fn validate_session_id(session_id: &str) -> Result<()> {
    let invalid = session_id.is_empty()
        || session_id == "."
        || session_id == ".."
        || session_id.contains("..")
        || session_id.ends_with('.')
        || session_id.ends_with(' ')
        || session_id.contains('/')
        || session_id.contains('\\')
        || session_id.contains('\0')
        || session_id.contains('<')
        || session_id.contains('>')
        || session_id.contains(':')
        || session_id.contains('"')
        || session_id.contains('|')
        || session_id.contains('?')
        || session_id.contains('*')
        || is_windows_reserved_device_name(session_id);

    if invalid {
        return Err(crate::error::Error::InvalidSessionId(
            session_id.to_string(),
        ));
    }

    Ok(())
}

fn is_windows_reserved_device_name(session_id: &str) -> bool {
    let upper = session_id.to_ascii_uppercase();
    let stem = upper.split('.').next().unwrap_or_default();

    is_windows_reserved_device_segment(&upper) || is_windows_reserved_device_segment(stem)
}

fn is_windows_reserved_device_segment(segment: &str) -> bool {
    matches!(segment, "CON" | "PRN" | "AUX" | "NUL")
        || matches!(segment.as_bytes(), [b'C', b'O', b'M', b'1'..=b'9'])
        || matches!(segment.as_bytes(), [b'L', b'P', b'T', b'1'..=b'9'])
}

#[cfg(test)]
mod tests {
    use super::validate_session_id;
    use crate::error::Error;

    #[test]
    fn reserved_device_names_with_extensions_are_invalid() {
        for session_id in ["CON.txt", "aux.log", "LPT1.json"] {
            assert!(matches!(
                validate_session_id(session_id),
                Err(Error::InvalidSessionId(ref value)) if value == session_id
            ));
        }
    }
}
