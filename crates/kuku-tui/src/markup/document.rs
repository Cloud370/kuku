use std::borrow::Cow;

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};

use super::OPTIONS;

const MIN_FENCE_LENGTH: usize = 3;
const REPARSE_BUDGET_FACTOR: usize = 3;

pub(super) fn is_markdown(info: &str) -> bool {
    info.split_whitespace().next().is_some_and(|word| {
        word.eq_ignore_ascii_case("markdown") || word.eq_ignore_ascii_case("md")
    })
}

pub(super) fn repair_fences(source: &str) -> Cow<'_, str> {
    let mut rest = source;
    let mut output = String::new();
    let mut budget = source.len().saturating_mul(REPARSE_BUDGET_FACTOR);
    while let Some(remaining) = budget.checked_sub(rest.len()) {
        if !rest.lines().any(is_document_opening) {
            break;
        }
        let Some(repair) = find_repair(rest) else {
            break;
        };
        budget = remaining;
        output.push_str(&rest[..repair.open]);
        output.push('`');
        output.push_str(&rest[repair.open..repair.close]);
        if repair.extend_close {
            output.push('`');
        }
        output.push_str(&rest[repair.close..repair.after]);
        rest = &rest[repair.after..];
    }
    if output.is_empty() {
        Cow::Borrowed(source)
    } else {
        output.push_str(rest);
        Cow::Owned(output)
    }
}

fn is_document_opening(line: &str) -> bool {
    Fence::parse(line).is_some_and(|fence| fence.is_triple_backtick() && is_markdown(fence.info))
}

fn find_repair(source: &str) -> Option<Repair> {
    for (event, range) in Parser::new_ext(source, OPTIONS).into_offset_iter() {
        let Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) = event else {
            continue;
        };
        if !is_markdown(&info) {
            continue;
        }
        let line_start = source[..range.start]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let Some((opening, body)) = source[line_start..].split_once('\n') else {
            break;
        };
        if !is_document_opening(opening) {
            continue;
        }
        match find_closing(source, source.len() - body.len()) {
            Closing::Standard => {}
            // Require a separate outer closer before overriding a standard fence boundary.
            Closing::Open => break,
            Closing::AfterInner { at, after, triple } => {
                return Some(Repair {
                    open: range.start,
                    close: at,
                    after,
                    extend_close: triple,
                });
            }
        }
    }
    None
}

fn find_closing(source: &str, mut start: usize) -> Closing {
    let mut inner: Option<Fence<'_>> = None;
    let mut passed_inner_closer = false;
    for line in source[start..].split_inclusive('\n') {
        let line_start = start;
        start += line.len();
        let Some(fence) = Fence::parse(line.trim_end_matches('\n')) else {
            continue;
        };
        if fence.marker == b'`' && fence.info.is_empty() {
            let triple = fence.is_triple_backtick();
            if triple && inner.is_some_and(|opening| opening.is_triple_backtick()) {
                passed_inner_closer = true;
                inner = None;
                continue;
            }
            return if passed_inner_closer {
                Closing::AfterInner {
                    at: line_start + fence.indent,
                    after: start,
                    triple,
                }
            } else {
                Closing::Standard
            };
        }

        match inner {
            Some(opening) if fence.closes(opening) => inner = None,
            None => inner = Some(fence),
            Some(_) => {}
        }
    }
    Closing::Open
}

struct Repair {
    open: usize,
    close: usize,
    after: usize,
    extend_close: bool,
}

enum Closing {
    Standard,
    Open,
    AfterInner {
        at: usize,
        after: usize,
        triple: bool,
    },
}

#[derive(Clone, Copy)]
struct Fence<'a> {
    marker: u8,
    length: usize,
    indent: usize,
    info: &'a str,
}

impl<'a> Fence<'a> {
    fn parse(line: &'a str) -> Option<Self> {
        let content = line.trim_start_matches(' ');
        let indent = line.len() - content.len();
        let marker = *content.as_bytes().first()?;
        if indent > 3 || !matches!(marker, b'`' | b'~') {
            return None;
        }
        let length = content.bytes().take_while(|&byte| byte == marker).count();
        let info = content[length..].trim_matches([' ', '\t']);
        if length < MIN_FENCE_LENGTH || (marker == b'`' && info.contains('`')) {
            return None;
        }
        Some(Self {
            marker,
            length,
            indent,
            info,
        })
    }

    fn is_triple_backtick(self) -> bool {
        self.marker == b'`' && self.length == MIN_FENCE_LENGTH
    }

    fn closes(self, opening: Self) -> bool {
        self.marker == opening.marker && self.length >= opening.length && self.info.is_empty()
    }
}
