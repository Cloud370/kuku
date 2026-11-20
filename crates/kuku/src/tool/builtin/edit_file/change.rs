use std::collections::HashMap;

use serde::Serialize;

const MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_CHANGED_LINES: usize = 256;
const MAX_PREVIEW_ROWS: usize = 8;
const MAX_PREVIEW_BYTES: usize = 2048;

#[derive(Serialize)]
pub(super) struct Change {
    added_lines: usize,
    removed_lines: usize,
    preview: String,
    preview_truncated: bool,
}

pub(super) fn summarize(before: &str, after: &str) -> Option<Change> {
    if before.len().saturating_add(after.len()) > MAX_INPUT_BYTES {
        return None;
    }
    let (before, after) = changed_region(before, after);
    let old: Vec<_> = before
        .split_inclusive('\n')
        .take(MAX_CHANGED_LINES + 1)
        .collect();
    let new: Vec<_> = after
        .split_inclusive('\n')
        .take(MAX_CHANGED_LINES + 1)
        .collect();
    if old.len().max(new.len()) > MAX_CHANGED_LINES {
        return None;
    }
    let width = new.len() + 1;
    let mut known = HashMap::new();
    let keys: Vec<_> = old
        .iter()
        .chain(&new)
        .map(|&line| {
            let next = known.len();
            *known.entry(line).or_insert(next)
        })
        .collect();
    let (old_keys, new_keys) = keys.split_at(old.len());
    let mut shared = vec![0u16; (old.len() + 1) * width];
    let cell = |i, j| i * width + j;
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            shared[cell(i, j)] = if old_keys[i] == new_keys[j] {
                1 + shared[cell(i + 1, j + 1)]
            } else {
                shared[cell(i + 1, j)].max(shared[cell(i, j + 1)])
            };
        }
    }
    let common = usize::from(shared[0]);
    let (mut i, mut j) = (0, 0);
    let mut preview = String::new();
    let mut rows = 0;
    let mut preview_truncated = false;
    while i < old.len() || j < new.len() {
        let (marker, line) = match (old.get(i), new.get(j)) {
            (Some(_), Some(_)) if old_keys[i] == new_keys[j] => {
                i += 1;
                j += 1;
                continue;
            }
            (None, Some(right)) => {
                j += 1;
                ('+', right)
            }
            (Some(_), Some(right)) if shared[cell(i, j + 1)] > shared[cell(i + 1, j)] => {
                j += 1;
                ('+', right)
            }
            (Some(left), _) => {
                i += 1;
                ('-', left)
            }
            (None, None) => break,
        };
        let room = MAX_PREVIEW_BYTES.saturating_sub(preview.len() + 2);
        if rows == MAX_PREVIEW_ROWS || room == 0 {
            preview_truncated = true;
            break;
        }
        if rows > 0 {
            preview.push('\n');
        }
        preview.push(marker);
        let line = line
            .strip_suffix("\r\n")
            .or_else(|| line.strip_suffix('\n'))
            .unwrap_or(line);
        let cut = line.floor_char_boundary(room.min(line.len()));
        preview.push_str(&line[..cut]);
        if cut != line.len() {
            preview_truncated = true;
            break;
        }
        rows += 1;
    }
    Some(Change {
        added_lines: new.len() - common,
        removed_lines: old.len() - common,
        preview,
        preview_truncated,
    })
}

fn changed_region<'a>(before: &'a str, after: &'a str) -> (&'a str, &'a str) {
    let matching = |a: &str, b: &str| a == b;
    let prefix: usize = before
        .split_inclusive('\n')
        .zip(after.split_inclusive('\n'))
        .take_while(|(a, b)| matching(a, b))
        .map(|(line, _)| line.len())
        .sum();
    let before = &before[prefix..];
    let after = &after[prefix..];
    let suffix: usize = before
        .split_inclusive('\n')
        .rev()
        .zip(after.split_inclusive('\n').rev())
        .take_while(|(a, b)| matching(a, b))
        .map(|(line, _)| line.len())
        .sum();
    (
        &before[..before.len() - suffix],
        &after[..after.len() - suffix],
    )
}

#[cfg(test)]
mod tests {
    use super::summarize;

    #[test]
    fn statistics_and_preview_follow_actual_disjoint_line_changes() {
        let change = summarize(
            "top\r\nalpha\r\nsame\r\nomega\r\nend\r\n",
            "top\r\n新\r\nsame\r\nend\r\n",
        )
        .unwrap();
        assert_eq!(1, change.added_lines);
        assert_eq!(2, change.removed_lines);
        assert_eq!("-alpha\n+新\n-omega", change.preview);
        assert!(!change.preview_truncated);
    }

    #[test]
    fn preview_stays_bounded_and_keeps_exact_statistics() {
        let after = format!("{}\n", "中".repeat(1600));
        let change = summarize("before\n", &after).unwrap();
        assert_eq!((1, 1), (change.added_lines, change.removed_lines));
        assert!(change.preview_truncated);
        assert!(change.preview.len() <= super::MAX_PREVIEW_BYTES);
        assert!(summarize("", &"extra\n".repeat(super::MAX_CHANGED_LINES + 1)).is_none());
    }
}
