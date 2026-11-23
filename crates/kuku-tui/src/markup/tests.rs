use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::{render_markdown, wrap_bounded};
use crate::theme::{
    ACCENT, CODE_BACKGROUND, CODE_COMMENT, CODE_KEYWORD, CODE_STRING, CODE_TEXT, MARKDOWN_EMPHASIS,
};

const RICH_TABLE: &str = "| 项目 | Result | Cost |\n\
                          | :--- | :---: | ---: |\n\
                          | 中文 | `a\\|b` | 7 |\n\
                          | 一 | x\\|y | 100 |";
const LIMIT: usize = 50;
const BEFORE_DOCUMENT: &str = "Before **example**\n";
const AFTER_DOCUMENT: &str = "After **example**";
const DOCUMENT: &str = concat!(
    "## 中文示例\n\n",
    "- 检查结果\n\n",
    "| 阶段 | 结果 |\n| --- | --- |\n| 隔离 | 通过 |\n\n",
    "```python\nreturn \"通过\"\n```\n\n",
    "```javascript\nconst total = 1;\n```\n\n",
    "### 附录\n",
);

fn rows(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect()
}

fn column_edges(line: &str) -> Vec<usize> {
    line.match_indices('│')
        .map(|(index, _)| line[..index].width())
        .collect()
}

#[test]
fn wrapped_lines_keep_role_emphasis_and_span_colors() {
    let line = Line::from(vec![Span::styled("甲乙", Style::new().fg(ACCENT))])
        .style(Style::new().add_modifier(Modifier::BOLD));
    for row in super::wrap(vec![line], 2) {
        assert!(row.spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(row.spans[0].style.fg, Some(ACCENT));
    }
}

#[test]
fn gfm_table_keeps_unicode_inline_code_and_escaped_pipes_in_aligned_columns() {
    let source = format!(
        "Before.\r\n\r\n{}\r\n\r\nAfter.",
        RICH_TABLE.replace('\n', "\r\n")
    );
    let lines = render_markdown(&source, 40, LIMIT);
    let plain = rows(&lines);
    let body: Vec<_> = plain.iter().filter(|line| line.starts_with('│')).collect();
    assert_eq!(3, body.len());
    assert_eq!(vec![0, 7, 16, 23], column_edges(body[0]));
    assert!(body
        .iter()
        .all(|line| column_edges(line) == column_edges(body[0])));
    assert_eq!("│ 中文 │  a|b   │    7 │", body[1]);
    assert_eq!("│ 一   │  x|y   │  100 │", body[2]);
    assert!(lines
        .iter()
        .flat_map(|line| &line.spans)
        .any(|span| { span.content == "a|b" && span.style.fg == Some(ACCENT) }));
    assert!(lines.iter().flat_map(|line| &line.spans).any(|span| {
        span.content == "项目" && span.style.add_modifier.contains(Modifier::BOLD)
    }));
    assert_eq!("Before.", plain.first().unwrap());
    assert!(plain.iter().any(|line| line == "After."));
}

#[test]
fn constrained_grid_wraps_long_cells_with_stable_column_edges() {
    let source = "| Key | Value |\n| --- | --- |\n| Id | `abcdefghijklmnopqrstuvwx` |";
    let width = 24;
    let plain = rows(&render_markdown(source, width, LIMIT));
    let body: Vec<_> = plain.iter().filter(|line| line.starts_with('│')).collect();
    assert_eq!(3, body.len());
    assert!(body.iter().all(|line| column_edges(line) == vec![0, 6, 23]));
    assert!(plain.iter().all(|line| line.width() <= width));
    let value: String = body[1..]
        .iter()
        .map(|line| line.split('│').nth(2).unwrap().trim())
        .collect();
    assert_eq!("abcdefghijklmnopqrstuvwx", value);
}

#[test]
fn narrow_table_keeps_fields_attached_to_each_logical_row() {
    let source =
        "| Id | 列 | Note |\n| --- | --- | --- |\n| one | 中 | `a\\|b` |\n| two | 文 | ok |";
    let width = 12;
    let plain = rows(&render_markdown(source, width, LIMIT));
    assert_eq!(
        [
            "Id: one",
            "列: 中",
            "Note: a|b",
            "",
            "Id: two",
            "列: 文",
            "Note: ok",
            ""
        ],
        plain.as_slice()
    );
    assert!(plain.iter().all(|line| line.width() <= width));
}

#[test]
fn quoted_table_stays_within_width_and_respects_the_row_limit() {
    let source = format!("> {}", RICH_TABLE.replace('\n', "\n> "));
    let width = 26;
    let limit = 5;
    let plain = rows(&wrap_bounded(
        render_markdown(&source, width, limit),
        width,
        limit,
    ));
    assert_eq!(limit + 1, plain.len());
    assert!(plain.first().unwrap().starts_with("│ ┌"));
    assert_eq!("…", plain.last().unwrap());
    assert!(plain.iter().all(|line| line.width() <= width));
}

#[test]
fn arriving_delimiters_reflow_partial_markdown_without_losing_text() {
    let width = 34;
    let mut source = String::from("Adjacent **marker");
    assert_eq!(
        "Adjacent **marker",
        rows(&render_markdown(&source, width, LIMIT))[0]
    );

    source.push_str("**\n\n| 列 | 值 |\n");
    let before_table = rows(&render_markdown(&source, width, LIMIT));
    assert!(before_table.iter().any(|line| line == "Adjacent marker"));
    assert!(before_table.iter().any(|line| line == "| 列 | 值 |"));

    source.push_str("| --- | --- |\n| 中文 | `a\\|b` |\n\n```rust\nlet 值");
    let before_fence = rows(&render_markdown(&source, width, LIMIT));
    assert!(before_fence.iter().any(|line| line.starts_with('┌')));
    assert!(before_fence.iter().any(|line| line.contains("中文 │ a|b")));
    assert!(before_fence
        .iter()
        .any(|line| line.starts_with("│ let 值 ")));
    assert!(before_fence.iter().any(|line| line.starts_with('╰')));

    source.push_str(" = 1;\n```\n\nFinished.");
    let complete = rows(&render_markdown(&source, width, LIMIT));
    assert_eq!(
        1,
        complete
            .iter()
            .filter(|line| line.starts_with("│ let 值 = 1; "))
            .count()
    );
    assert_eq!(
        1,
        complete.iter().filter(|line| *line == "Finished.").count()
    );
}

#[test]
fn pipe_text_and_fenced_code_keep_their_markdown_meaning() {
    let source = "| 项目 | 值 | | --- | --- | | A | B |";
    let plain = rows(&render_markdown(source, 60, LIMIT));
    assert_eq!([source, ""], plain.as_slice());

    let fenced = format!("```text\n{RICH_TABLE}\n```\n\nOrdinary\nparagraph.");
    let plain = rows(&render_markdown(&fenced, 60, LIMIT));
    assert!(plain[0].starts_with("╭─ text "));
    assert!(plain
        .iter()
        .any(|line| line.starts_with("│ | 项目 | Result | Cost | ")));
    assert!(plain.iter().any(|line| line == "Ordinary paragraph."));
    assert!(plain.iter().all(|line| !line.starts_with('┌')));
}

#[test]
fn unlabeled_fences_preserve_literal_content_and_wrap_without_a_language_title() {
    let source = "```\n  # 甲乙\n\n    **Literal**\n```";
    let width = 20;
    let plain = rows(&render_markdown(source, width, LIMIT));
    assert!(plain[0].starts_with("╭─ 3 lines "));
    assert_eq!(width, plain[0].width());
    assert!(plain[1].starts_with("│   # 甲乙 "));
    assert_eq!(format!("│{}│", " ".repeat(width - 2)), plain[2]);
    assert!(plain[3].starts_with("│     **Literal** "));
    assert_eq!(
        ["  a", "bcd", ""],
        rows(&render_markdown("```\n  abcd\n```", 3, LIMIT)).as_slice(),
    );
}

#[test]
fn an_empty_streaming_fence_waits_for_content_or_an_explicit_language() {
    for source in ["```", "```\n", "```\n```", "~~~  \n"] {
        assert!(render_markdown(source, 32, LIMIT).is_empty(), "{source:?}");
    }
    assert_eq!(
        render_markdown("Intro.", 32, LIMIT),
        render_markdown("Intro.\n```", 32, LIMIT),
    );
    let explicit = rows(&render_markdown("```python", 32, LIMIT));
    assert!(explicit[0].starts_with("╭─ python "));
    assert!(explicit[0].starts_with("╭─ python · 0 lines "));
    let unfinished = rows(&render_markdown("```python\nfirst\n\nlast", 32, LIMIT));
    assert!(unfinished[0].starts_with("╭─ python · 3 lines "));
}

#[test]
fn indented_code_keeps_relative_indentation_under_an_untitled_border() {
    let width = 32;
    let plain = rows(&render_markdown(
        "    # Literal\n        second",
        width,
        LIMIT,
    ));
    assert!(plain[0].starts_with("╭─ 2 lines "));
    assert!(plain[2].starts_with("│ # Literal "));
    assert!(plain[3].starts_with("│     second "));
}

#[test]
fn fence_labels_and_highlights_lexical_scopes_across_lines() {
    let source = concat!(
        "```rust linenos\n",
        "let greeting = \"let 中文\";\n",
        "/* let 42\n",
        "   let still a comment */\n",
        "let answer = 42;\n",
        "```",
    );
    let lines = render_markdown(source, 48, LIMIT);
    let plain = rows(&lines);
    assert!(plain[0].starts_with("╭─ rust "));
    assert!(!plain[0].contains("linenos"));
    assert!(plain[4].starts_with("│    let still a comment */"));
    assert!(plain[6].trim_matches('│').trim().is_empty());
    assert!(plain[7].starts_with('╰'));
    assert!(lines[..8].iter().all(|line| line.width() == 48));
    assert!(lines[..8].iter().all(|line| line
        .spans
        .iter()
        .all(|span| { line.style.patch(span.style).bg == Some(CODE_BACKGROUND) })));
    for (row, part, color) in [
        (2, "let", CODE_KEYWORD),
        (2, "let 中文", CODE_STRING),
        (3, "let 42", CODE_COMMENT),
        (4, "let still a comment", CODE_COMMENT),
        (5, "let", CODE_KEYWORD),
    ] {
        assert!(
            lines[row]
                .spans
                .iter()
                .any(|span| { span.content.contains(part) && span.style.fg == Some(color) }),
            "missing color for {part:?} in {:?}",
            lines[row]
        );
    }
}

#[test]
fn code_title_counts_source_lines_before_wrapping_or_clipping_body() {
    let source =
        "```text\nThis source line will wrap across several rows of a narrow terminal.\n```";
    for width in [20, 52] {
        let lines = render_markdown(source, width, 1);
        let title = lines.first().unwrap();
        assert_eq!(width, title.width());
        assert!(title.to_string().starts_with("╭─ text · 1 line "));
    }
    let narrow = render_markdown(source, 8, LIMIT);
    assert!(narrow[0].to_string().contains("text"));
    assert!(narrow.iter().all(|line| line.width() <= 8));
}

#[test]
fn unknown_fences_and_long_lines_keep_plain_code_and_independent_scopes() {
    let long = "/".repeat(1100);
    let source = format!(
        concat!(
            "```unknown_lang\nlet unknown = 3;\n```\n\n",
            "```rust\n{}\nlet after_long = 4;\n```\n\n",
            "```python\nreturn 5\n```",
        ),
        long
    );
    let lines = render_markdown(&source, 80, LIMIT);
    assert!(lines[0].to_string().starts_with("╭─ unknown_lang "));
    for word in ["let unknown", "let after_long"] {
        assert!(lines
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| { span.content.contains(word) && span.style.fg == Some(CODE_TEXT) }));
    }
    assert!(lines
        .iter()
        .flat_map(|line| &line.spans)
        .any(|span| { span.content == "return" && span.style.fg == Some(CODE_KEYWORD) }));
}

#[test]
fn quoted_code_wraps_original_unicode_graphemes_at_narrow_widths() {
    let source = "> ```text\n>  中文👩‍💻𠮷字\n> ```";
    for width in 4..17 {
        let lines = render_markdown(source, width, LIMIT);
        let plain = rows(&lines);
        assert!(lines.iter().all(|line| line.width() <= width));
        assert!(plain.iter().all(|line| !line.contains('�')));
        let body: String = plain
            .iter()
            .skip(1)
            .take_while(|line| !line.is_empty() && !line.contains('╰'))
            .flat_map(|line| line.chars())
            .filter(|ch| *ch != '│' && !ch.is_whitespace())
            .collect();
        assert_eq!("中文👩‍💻𠮷字", body, "width {width}");
    }
}

#[test]
fn headings_use_spacing_and_typography() {
    let lines = render_markdown("# First *part*\n\n### 中文 `config`\n\nBody", 30, LIMIT);
    assert_eq!(
        ["First part", "", "中文 config", "", "Body", ""],
        rows(&lines).as_slice()
    );
    assert!(lines[0]
        .spans
        .iter()
        .all(|span| span.style.add_modifier.contains(Modifier::BOLD)));
    assert!(lines[2].spans.iter().all(|span| {
        span.style.add_modifier.contains(Modifier::BOLD)
            && !span.style.add_modifier.contains(Modifier::ITALIC)
    }));
    assert!(lines[4].spans[0].style.add_modifier.is_empty());
}

#[test]
fn emphasis_keeps_the_transcript_background_in_wrapped_quotes() {
    let markdown =
        "Plain *English emphasis* and **strong**.\n\n> *中文强调 `inline_code` more 中文*";
    for width in [19, 64] {
        let lines = render_markdown(markdown, width, LIMIT);
        let spans: Vec<_> = lines.iter().flat_map(|line| &line.spans).collect();
        let emphasized: String = spans
            .iter()
            .filter(|span| span.style.fg == Some(MARKDOWN_EMPHASIS))
            .map(|span| span.content.as_ref())
            .collect();
        assert!(emphasized.contains("English emphasis"));
        assert!(emphasized.contains("中文强调"));
        assert!(emphasized.contains("more"));
        if crossterm::style::Colored::ansi_color_disabled_memoized() {
            assert!(spans
                .iter()
                .filter(|span| span.style.fg == Some(MARKDOWN_EMPHASIS))
                .all(|span| span.style.add_modifier.contains(Modifier::UNDERLINED)));
        }
        assert!(spans.iter().all(|span| {
            span.style.bg.is_none()
                && !span
                    .style
                    .add_modifier
                    .intersects(Modifier::ITALIC | Modifier::REVERSED)
        }));
        let inline_code: String = spans
            .iter()
            .filter(|span| span.style.fg == Some(ACCENT))
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(inline_code, "inline_code");
        assert!(spans.iter().any(|span| span.content.contains("strong")
            && span.style.add_modifier.contains(Modifier::BOLD)));
        assert!(rows(&lines).iter().any(|row| row.starts_with("│ ")));
    }
}

fn document(fence: &str, language: &str) -> String {
    format!("{BEFORE_DOCUMENT}{fence}{language}\n{DOCUMENT}{fence}\n{AFTER_DOCUMENT}")
}

#[test]
fn labeled_markdown_fences_preview_headings_tables_lists_and_language_code() {
    for (fence, language) in [("````", "markdown title=sample"), ("~~~", "MD")] {
        let source = document(fence, language);
        let lines = render_markdown(&source, 40, LIMIT);
        let plain = rows(&lines);
        assert_eq!("Before example", plain.first().unwrap());
        assert!(plain.iter().any(|line| line == "After example"));
        assert!(plain.iter().any(|line| line
            == &format!(
                "Markdown preview · Messages · {} lines",
                DOCUMENT.lines().count()
            )));
        assert!(plain.iter().any(|line| line.starts_with('┌')));
        assert!(plain.iter().any(|line| line.contains("隔离 │ 通过")));
        assert!(plain.iter().any(|line| line == "• 检查结果"));
        assert!(plain.iter().any(|line| line == "附录"));
        assert!(plain.iter().any(|line| line.starts_with("╭─ python ")));
        assert!(plain.iter().any(|line| line.starts_with("╭─ javascript ")));
        assert!(plain.iter().all(|line| !line.contains("```")));
        assert!(lines.iter().flat_map(|line| &line.spans).any(|span| {
            span.content == "中文示例" && span.style.add_modifier.contains(Modifier::BOLD)
        }));
        for keyword in ["return", "const"] {
            assert!(
                lines
                    .iter()
                    .flat_map(|line| &line.spans)
                    .any(|span| { span.content == keyword && span.style.fg == Some(CODE_KEYWORD) }),
                "missing the language-specific style for {keyword}"
            );
        }

        let width = 12;
        let narrow = wrap_bounded(render_markdown(&source, width, LIMIT), width, LIMIT);
        let narrow_rows = rows(&narrow);
        assert!(narrow.iter().all(|line| line.width() <= width));
        assert!(narrow_rows.iter().any(|line| line == "阶段: 隔离"));
        assert!(narrow_rows.iter().any(|line| line == "结果: 通过"));
    }
}

#[test]
fn same_length_labeled_fences_reflow_a_streamed_document_at_its_actual_end() {
    let source = document("```", "markdown");
    for prefix in [
        source.split("return").next().unwrap(),
        source.split("const").next().unwrap(),
        source.split("### 附录").next().unwrap(),
    ] {
        let plain = rows(&render_markdown(prefix, 44, LIMIT));
        assert_eq!(
            1,
            plain
                .iter()
                .filter(|line| line.as_str() == "中文示例")
                .count()
        );
        assert_eq!(
            1,
            plain
                .iter()
                .filter(|line| line.starts_with("╭─ python "))
                .count()
        );
        assert!(plain.iter().any(|line| line.contains("隔离 │ 通过")));
    }

    let expected = document("````", "markdown");
    assert_eq!(
        render_markdown(&expected, 44, LIMIT),
        render_markdown(&source, 44, LIMIT),
    );
    let adjacent = format!("{source}\n\n{source}");
    let expected = format!("{expected}\n\n{expected}");
    assert_eq!(
        render_markdown(&expected, 44, 2 * LIMIT),
        render_markdown(&adjacent, 44, 2 * LIMIT),
    );
}

#[test]
fn code_cards_share_the_right_edge_and_honor_the_list_or_quote_gutter() {
    for width in [26, 74, 142] {
        for (source, prefix) in [
            ("```rust\nfn main() {}\n```", "╭─ rust"),
            ("- item\n\n  ```rust\n  fn main() {}\n  ```", "  ╭─ rust"),
            (
                "12. item\n\n    ```rust\n    fn main() {}\n    ```",
                "    ╭─ rust",
            ),
            (
                "> - item\n>\n>   ```rust\n>   fn main() {}\n>   ```",
                "│   ╭─ rust",
            ),
        ] {
            let rows = render_markdown(source, width, LIMIT);
            let frame = rows
                .iter()
                .find(|line| line.to_string().contains("╭─ rust"))
                .unwrap();
            assert!(frame.to_string().starts_with(prefix));
            assert_eq!(width, frame.width());
        }
    }
}

#[test]
fn incomplete_standard_document_keeps_its_nested_code_inside_the_quote_width() {
    let open = "> ````markdown\n> # 中文\n>\n> ```rust\n> let 值 = 7;";
    let width = 26;
    let streaming = render_markdown(open, width, LIMIT);
    let plain = rows(&streaming);
    assert!(plain.iter().any(|line| line == "│ 中文"));
    assert!(plain.iter().any(|line| line.starts_with("│ ╭─ rust ")));
    assert!(plain.iter().any(|line| line.starts_with("│ │ let 值 = 7;")));
    assert!(streaming.iter().all(|line| line.width() <= width));

    let complete = format!("{open}\n> ```\n> ````");
    assert_eq!(streaming, render_markdown(&complete, width, LIMIT));
}

#[test]
fn ordinary_code_and_a_second_document_layer_keep_their_literal_content() {
    let source = concat!(
        "````text\n```markdown\n# Source, not a heading\n```\n````\n\n",
        "~~~~markdown\n# Rendered heading\n\n",
        "````md\n# Inner source\n````\n~~~~",
    );
    let plain = rows(&render_markdown(source, 50, LIMIT));
    assert_eq!(
        1,
        plain
            .iter()
            .filter(|line| line.contains("Markdown preview"))
            .count()
    );
    assert!(plain.iter().any(|line| line.contains("│ ```markdown ")));
    assert!(plain
        .iter()
        .any(|line| line.contains("│ # Source, not a heading ")));
    assert!(plain.iter().any(|line| line == "Rendered heading"));
    assert!(plain.iter().any(|line| line.contains("│ # Inner source ")));
}

#[test]
fn nested_document_uses_the_reply_highlighting_and_row_budgets() {
    let source = format!(
        "```rust\n{}```\n\n````markdown\n```rust\nlet inside = 7;\n```\n````",
        "let before = 1;\n".repeat(128)
    );
    let lines = render_markdown(&source, 40, 160);
    assert!(lines
        .iter()
        .flat_map(|line| &line.spans)
        .any(|span| { span.content == "let" && span.style.fg == Some(CODE_KEYWORD) }));
    assert!(lines.iter().flat_map(|line| &line.spans).any(|span| {
        span.content.contains("let inside = 7;") && span.style.fg == Some(CODE_TEXT)
    }));

    let source = format!("~~~~md\n{}~~~~", "## 长文\n\n".repeat(300));
    let limit = 5;
    let lines = render_markdown(&source, 40, limit);
    assert_eq!(limit + 1, lines.len());
    assert_eq!("…", rows(&wrap_bounded(lines, 40, limit)).last().unwrap());
}
