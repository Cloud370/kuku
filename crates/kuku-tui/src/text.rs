#[cfg(windows)]
use std::ffi::OsString;
use std::path::Path;
#[cfg(windows)]
use std::path::{Component, PathBuf, Prefix};

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub(crate) const TAB: &str = "    ";

pub(crate) fn terminal_text(text: &str) -> String {
    let mut safe = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\n' | '\t' => safe.push(character),
            '\u{0000}'..='\u{001f}' => {
                safe.push(char::from_u32(0x2400 + u32::from(character)).expect("control picture"));
            }
            '\u{007f}' => safe.push('\u{2421}'),
            '\u{0080}'..='\u{009f}' => safe.push('\u{fffd}'),
            _ => safe.push(character),
        }
    }
    safe
}

pub(crate) fn single_line(text: &str) -> String {
    terminal_text(text).replace(['\n', '\t'], " ")
}

pub(crate) fn single_line_path(path: &Path) -> String {
    #[cfg(windows)]
    let ordinary = ordinary_windows_path(path);
    #[cfg(windows)]
    let path = ordinary.as_deref().unwrap_or(path);
    single_line(&path.to_string_lossy())
}

pub(crate) fn replace_tabs(text: &str) -> String {
    text.replace('\t', TAB)
}

pub(crate) fn cap(text: &str, bytes: usize) -> &str {
    &text[..text.floor_char_boundary(bytes.min(text.len()))]
}

pub(crate) fn shorten(text: &str, cells: usize) -> String {
    if text.width() <= cells {
        return text.to_owned();
    }
    if cells == 0 {
        return String::new();
    }
    let mut shortened = String::new();
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        used += grapheme.width();
        if used >= cells {
            break;
        }
        shortened.push_str(grapheme);
    }
    shortened.push('…');
    shortened
}

#[cfg(windows)]
fn ordinary_windows_path(path: &Path) -> Option<PathBuf> {
    let mut components = path.components();
    let Component::Prefix(prefix) = components.next()? else {
        return None;
    };
    let mut ordinary = match prefix.kind() {
        Prefix::VerbatimDisk(drive) => PathBuf::from(format!("{}:", char::from(drive))),
        Prefix::VerbatimUNC(server, share) => {
            let mut unc = OsString::from(r"\\");
            unc.push(server);
            unc.push(r"\");
            unc.push(share);
            PathBuf::from(unc)
        }
        Prefix::Disk(_) | Prefix::UNC(_, _) | Prefix::DeviceNS(_) | Prefix::Verbatim(_) => {
            return None;
        }
    };
    ordinary.extend(components);
    Some(ordinary)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::single_line_path;

    #[cfg(windows)]
    #[test]
    fn windows_path_display_simplifies_only_verbatim_disks_and_unc() {
        let cases = [
            (r"\\?\C:\my workspace", r"C:\my workspace"),
            (r"\\?\UNC\server\share\project", r"\\server\share\project"),
            (r"\\.\pipe\kuku", r"\\.\pipe\kuku"),
            (r"\\?\Volume{volume}\project", r"\\?\Volume{volume}\project"),
        ];
        for (original, expected) in cases {
            assert_eq!(expected, single_line_path(Path::new(original)));
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_path_display_keeps_literal_backslashes() {
        let path = r"/workspace/\\?\C:\real-name";
        assert_eq!(path, single_line_path(Path::new(path)));
    }
}
