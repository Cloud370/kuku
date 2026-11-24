use std::borrow::Cow;
use std::sync::LazyLock;

use regex::bytes::Regex;

static COLOR_CODES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;:]*m").expect("valid ANSI color pattern"));

pub(crate) fn display_text(
    bytes: &[u8],
    starts_line: bool,
    ends_stream: bool,
) -> (Cow<'_, str>, bool) {
    if bytes.contains(&0x1b) {
        if let Cow::Owned(plain) = COLOR_CODES.replace_all(bytes, &b""[..]) {
            let (text, lossy) = decode(&plain, starts_line, ends_stream);
            return (Cow::Owned(text.into_owned()), lossy);
        }
    }
    decode(bytes, starts_line, ends_stream)
}

#[cfg(not(windows))]
fn decode(bytes: &[u8], _: bool, _: bool) -> (Cow<'_, str>, bool) {
    let text = String::from_utf8_lossy(bytes);
    let lossy = matches!(text, Cow::Owned(_));
    (text, lossy)
}

#[cfg(windows)]
fn decode(bytes: &[u8], mut starts_line: bool, ends_stream: bool) -> (Cow<'_, str>, bool) {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return (Cow::Borrowed(text), false);
    }
    let mut text = String::with_capacity(bytes.len());
    let mut lossy = false;
    for line in bytes.split_inclusive(|&byte| byte == b'\n') {
        let ends_line = line.ends_with(b"\n");
        if let Ok(utf8) = std::str::from_utf8(line) {
            text.push_str(utf8);
        } else if let Some(native) = (starts_line && (ends_line || ends_stream))
            .then(|| decode_oem(line))
            .flatten()
        {
            text.push_str(&native);
        } else {
            text.push_str(&String::from_utf8_lossy(line));
            lossy = true;
        }
        starts_line = ends_line;
    }
    (Cow::Owned(text), lossy)
}

#[cfg(windows)]
fn decode_oem(bytes: &[u8]) -> Option<String> {
    use windows_sys::Win32::Globalization::{CP_OEMCP, MB_ERR_INVALID_CHARS, MultiByteToWideChar};

    if bytes
        .iter()
        .any(|&byte| byte.is_ascii_control() && !matches!(byte, b'\r' | b'\n' | b'\t'))
    {
        return None;
    }
    let size = i32::try_from(bytes.len()).ok()?;
    let mut buffer = vec![0u16; bytes.len()];
    // SAFETY: Both buffers live through the call, with size bounded in bytes and UTF-16 units respectively.
    let written = unsafe {
        MultiByteToWideChar(
            CP_OEMCP,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            size,
            buffer.as_mut_ptr(),
            size,
        )
    };
    if written == 0 {
        return None;
    }
    String::from_utf16(&buffer[..written as usize]).ok()
}
