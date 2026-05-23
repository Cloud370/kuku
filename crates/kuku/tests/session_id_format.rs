use kuku::session::new_session_id;

#[test]
fn new_session_id_matches_expected_format() {
    let id = new_session_id();
    // YYYYMMDD-HHmm-xxxx = 8+1+4+1+4 = 18 chars
    assert_eq!(id.len(), 18, "expected 18 chars, got '{id}'");
    assert_eq!(id.chars().nth(8), Some('-'));
    assert_eq!(id.chars().nth(13), Some('-'));

    // date part: YYYYMMDD, all digits
    let date = &id[0..8];
    assert!(date.chars().all(|c| c.is_ascii_digit()));
    let year: u32 = date[0..4].parse().unwrap();
    assert!(year >= 2026, "year {year} should be >= 2026");

    // time part: HHmm, all digits
    let hour: u32 = id[9..11].parse().unwrap();
    assert!(hour < 24, "hour {hour} should be < 24");
    let minute: u32 = id[11..13].parse().unwrap();
    assert!(minute < 60, "minute {minute} should be < 60");

    // hex suffix: 4 lowercase hex chars
    let suffix = &id[14..18];
    assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(suffix.len(), 4);
}

#[test]
fn new_session_id_generates_unique_values() {
    let mut ids = std::collections::HashSet::new();
    for _ in 0..100 {
        assert!(ids.insert(kuku::session::new_session_id()));
    }
}
