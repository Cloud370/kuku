use std::io::Write;

use super::*;

fn request(offset: u64, limit: usize) -> OutputRead {
    OutputRead {
        offset,
        limit,
        search: None,
    }
}

fn text(chunk: &OutputChunk) -> String {
    chunk
        .segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect()
}

#[tokio::test]
async fn output_round_trip_preserves_stream_order_and_byte_ranges() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = OutputWriter::create(dir.path(), "call-1", OutputLimits::default())
        .await
        .unwrap();
    writer.write(OutputStream::Stdout, b"hello ").await.unwrap();
    writer.write(OutputStream::Stderr, b"world").await.unwrap();
    writer.write(OutputStream::Stdout, b"!\n").await.unwrap();
    writer.finish(false).await.unwrap();
    let chunk = read_output(dir.path(), "call-1", request(0, 100)).unwrap();
    assert_eq!("hello world!\n", text(&chunk));
    assert_eq!(
        vec![
            OutputStream::Stdout,
            OutputStream::Stderr,
            OutputStream::Stdout
        ],
        chunk
            .segments
            .iter()
            .map(|segment| segment.stream)
            .collect::<Vec<_>>()
    );
    assert_eq!(13, chunk.total_bytes);
    assert!(chunk.complete);
    assert!(!chunk.capture_truncated);
    let chunk = read_output(dir.path(), "call-1", request(6, 5)).unwrap();
    assert_eq!("world", text(&chunk));
    assert_eq!(11, chunk.next_offset);
    assert!(chunk.has_more);
    let last = read_output(dir.path(), "call-1", request(chunk.next_offset, 5)).unwrap();
    assert_eq!("!\n", text(&last));
    assert!(!last.has_more);
}

#[tokio::test]
async fn output_quota_includes_headers_and_retains_observed_summary() {
    let dir = tempfile::tempdir().unwrap();
    let limits = OutputLimits {
        max_output_bytes: (HEADER_BYTES + FRAME_HEADER_BYTES + 7) as u64,
        max_preview_bytes: 100,
    };
    let mut writer = OutputWriter::create(dir.path(), "quota", limits)
        .await
        .unwrap();
    writer
        .write(OutputStream::Stdout, b"123456789\n")
        .await
        .unwrap();
    writer
        .write(
            OutputStream::Stderr,
            b"test result: ok. 3 passed; 0 failed\n",
        )
        .await
        .unwrap();
    let metadata = writer.finish(false).await.unwrap();
    assert_eq!(7, metadata.stored_bytes);
    assert_eq!(10, metadata.stdout_bytes);
    assert!(metadata.quota_truncated);
    assert_eq!(
        limits.max_output_bytes,
        fs::metadata(dir.path().join("quota.output")).unwrap().len()
    );
    let chunk = read_output(dir.path(), "quota", request(0, 100)).unwrap();
    assert_eq!("1234567", text(&chunk));
    assert!(chunk.capture_truncated);
    assert!(chunk.summary[0].contains("3 passed"));
}

#[tokio::test]
async fn output_search_handles_frame_and_incremental_window_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = OutputWriter::create(dir.path(), "search", OutputLimits::default())
        .await
        .unwrap();
    writer
        .write(OutputStream::Stdout, b"hello wor")
        .await
        .unwrap();
    writer
        .write(OutputStream::Stdout, b"ld and goodbye")
        .await
        .unwrap();
    writer.finish(false).await.unwrap();
    let first = read_output(
        dir.path(),
        "search",
        OutputRead {
            offset: 0,
            limit: 8,
            search: Some("world".to_string()),
        },
    )
    .unwrap();
    assert!(first.segments.is_empty());
    assert!(first.filtered);
    assert_eq!(4, first.next_offset);
    let next = read_output(
        dir.path(),
        "search",
        OutputRead {
            offset: first.next_offset,
            limit: 8,
            search: Some("world".to_string()),
        },
    )
    .unwrap();
    assert!(text(&next).contains("world"));
    assert!(next.next_offset > first.next_offset);
    let absent = read_output(
        dir.path(),
        "search",
        OutputRead {
            offset: 0,
            limit: 100,
            search: Some("absent".to_string()),
        },
    )
    .unwrap();
    assert!(absent.segments.is_empty());
    assert!(!absent.has_more);
}

#[tokio::test]
async fn output_preserves_raw_bytes_and_reports_lossy_slices() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = OutputWriter::create(dir.path(), "unicode", OutputLimits::default())
        .await
        .unwrap();
    let bytes = "你好".as_bytes();
    writer
        .write(OutputStream::Stdout, &bytes[..2])
        .await
        .unwrap();
    writer
        .write(OutputStream::Stdout, &bytes[2..])
        .await
        .unwrap();
    let invalid = b"\xff\0\r";
    writer.write(OutputStream::Stderr, invalid).await.unwrap();
    writer.finish(false).await.unwrap();
    let chunk = read_output(dir.path(), "unicode", request(0, bytes.len())).unwrap();
    assert_eq!("你好", text(&chunk));
    assert!(!chunk.lossy_utf8);
    assert!(chunk.segments[0].raw_bytes.is_none());
    let partial = read_output(dir.path(), "unicode", request(1, 2)).unwrap();
    assert!(partial.lossy_utf8);
    assert_eq!(3, partial.next_offset);
    assert_eq!(&bytes[1..3], partial.segments[0].as_bytes());
    let rest = read_output(dir.path(), "unicode", request(partial.next_offset, 100)).unwrap();
    assert_eq!(&bytes[3..], rest.segments[0].as_bytes());
    assert_eq!(invalid.as_slice(), rest.segments[1].as_bytes());
    let wire = serde_json::to_value(&rest).unwrap();
    assert!(wire["segments"][0].get("raw_bytes").is_none());
    assert_eq!(
        serde_json::json!([255, 0, 13]),
        wire["segments"][1]["raw_bytes"]
    );
}

#[tokio::test]
async fn output_rejects_escape_ids_and_never_overwrites_existing_output() {
    let dir = tempfile::tempdir().unwrap();
    for id in ["", "../escape", "a/b", "a\\b", ".", "..", "💥"] {
        assert!(
            OutputWriter::create(dir.path(), id, OutputLimits::default())
                .await
                .is_err(),
            "unsafe id {id:?} was accepted"
        );
        assert!(read_output(dir.path(), id, request(0, 10)).is_err());
    }
    let writer = OutputWriter::create(dir.path(), "same", OutputLimits::default())
        .await
        .unwrap();
    writer.finish(false).await.unwrap();
    assert_eq!(
        io::ErrorKind::AlreadyExists,
        OutputWriter::create(dir.path(), "same", OutputLimits::default())
            .await
            .err()
            .unwrap()
            .kind()
    );
    assert!(
        read_output(dir.path(), "same", request(0, 100))
            .unwrap()
            .complete
    );
}

#[tokio::test]
async fn output_detects_committed_corruption_and_bounds_read_requests() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = OutputWriter::create(dir.path(), "corrupt", OutputLimits::default())
        .await
        .unwrap();
    writer.write(OutputStream::Stdout, b"hello").await.unwrap();
    writer.finish(false).await.unwrap();
    assert!(read_output(dir.path(), "corrupt", request(0, 0)).is_err());
    assert!(read_output(dir.path(), "corrupt", request(0, MAX_READ_BYTES + 1)).is_err());
    let mut file = fs::OpenOptions::new()
        .write(true)
        .open(dir.path().join("corrupt.output"))
        .unwrap();
    file.seek(SeekFrom::Start(HEADER_BYTES as u64 + 1)).unwrap();
    file.write_all(&u32::MAX.to_le_bytes()).unwrap();
    assert!(read_output(dir.path(), "corrupt", request(0, 10)).is_err());
}

#[tokio::test]
async fn unfinished_capture_is_readable_and_marked_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = OutputWriter::create(dir.path(), "partial", OutputLimits::default())
        .await
        .unwrap();
    writer
        .write(OutputStream::Stdout, b"partial\n")
        .await
        .unwrap();
    writer.file.flush().await.unwrap();
    let chunk = read_output(dir.path(), "partial", request(0, 100)).unwrap();
    assert!(!chunk.complete);
    assert_eq!("partial\n", text(&chunk));
    drop(writer);
    fs::OpenOptions::new()
        .append(true)
        .open(dir.path().join("partial.output"))
        .unwrap()
        .write_all(&[1, 10])
        .unwrap();
    assert_eq!(
        "partial\n",
        text(&read_output(dir.path(), "partial", request(0, 100)).unwrap())
    );
}
