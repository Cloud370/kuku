#[cfg(windows)]
use std::ffi::OsString;
use std::path::Path;
#[cfg(windows)]
use std::path::{Component, PathBuf, Prefix};
use std::time::Duration;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub(crate) const TAB: &str = "    ";
const TOKEN_UNITS: [(u64, &str); 3] = [(1_000_000_000, "B"), (1_000_000, "M"), (1_000, "K")];

pub(crate) fn terminal_text(text: &str) -> String {
    let mut safe = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' if characters.peek() == Some(&'\n') => {}
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

pub(crate) fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let seconds = seconds % 60;
    match (hours, minutes) {
        (0, 0) => format!("{seconds}s"),
        (0, _) => format!("{minutes}m {seconds:02}s"),
        _ => format!("{hours}h {:02}m {seconds:02}s", minutes % 60),
    }
}

pub(crate) fn compact_token_count(count: u64) -> String {
    let Some((unit, suffix)) = TOKEN_UNITS.into_iter().find(|(unit, _)| *unit <= count) else {
        return count.to_string();
    };
    let whole = count / unit;
    if whole >= 100 {
        return format!("{whole}{suffix}");
    }
    let (precision, scale) = if whole < 10 { (2, 100) } else { (1, 10) };
    let fractional = count % unit / (unit / scale);
    if fractional == 0 {
        return format!("{whole}{suffix}");
    }
    let decimals = format!("{fractional:0precision$}");
    format!("{whole}.{}{suffix}", decimals.trim_end_matches('0'))
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

pub(crate) fn shorten_middle(text: &str, width: usize) -> String {
    if text.width() <= width || width < 8 {
        return shorten(text, width);
    }
    let (tail, used) = tail_start(text, width / 2);
    format!("{}{}", shorten(&text[..tail], width - used), &text[tail..])
}

pub(crate) fn with_suffix(text: &str, suffix: &str, width: usize) -> String {
    let separator = " · ";
    let suffix_width = suffix.width();
    if width <= suffix_width + separator.width() {
        return shorten(suffix, width);
    }
    let text = shorten_middle(text, width - suffix_width - separator.width());
    format!("{text}{separator}{suffix}")
}

pub(crate) fn effort_label(effort: Option<&str>) -> String {
    let default = kuku::config::ThinkLevel::ProviderDefault.as_str();
    let Some(effort) = effort.filter(|effort| *effort != default) else {
        return "Default".into();
    };
    let effort = single_line(effort);
    let mut chars = effort.chars();
    chars.next().map_or(effort.clone(), |first| {
        format!("{}{}", first.to_uppercase(), chars.as_str())
    })
}

pub(crate) fn tail_start(text: &str, width: usize) -> (usize, usize) {
    let mut used = 0;
    let mut tail = text.len();
    for (offset, grapheme) in text.grapheme_indices(true).rev() {
        let cells = grapheme.width();
        if used + cells > width {
            break;
        }
        used += cells;
        tail = offset;
    }
    (tail, used)
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
    use std::time::Duration;

    use super::{format_elapsed, single_line_path};

    #[test]
    fn elapsed_keeps_whole_seconds_and_pads_groups() {
        for (ms, expected) in [
            (999, "0s"),
            (8_999, "8s"),
            (65_000, "1m 05s"),
            (3_954_999, "1h 05m 54s"),
        ] {
            assert_eq!(expected, format_elapsed(Duration::from_millis(ms)));
        }
    }

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
