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
