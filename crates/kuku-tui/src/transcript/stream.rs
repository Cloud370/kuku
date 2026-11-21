use ratatui::text::Line;

use crate::markup::MarkdownStream;

use super::preview::{assistant_body, assistant_header, assistant_width};
use super::{Row, MAX_VISUAL_ROWS};

#[derive(Debug, Default)]
pub(super) struct StreamingReply {
    layout: Option<(usize, bool)>,
    parsed_bytes: usize,
    stable_rows: usize,
    heading_rows: usize,
    markdown: MarkdownStream,
    rows: Vec<Row>,
}

impl StreamingReply {
    #[cfg(test)]
    pub(super) fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub(super) fn render(&mut self, text: &str, thinking: bool, width: usize) -> &[Row] {
        let layout = (width, thinking);
        if self.layout != Some(layout) {
            let rows = assistant_header(thinking, true, true, width);
            *self = Self {
                layout: Some(layout),
                heading_rows: rows.len(),
                rows,
                ..Self::default()
            };
        }
        if self.parsed_bytes == text.len() || self.stable_rows > MAX_VISUAL_ROWS {
            return &self.rows;
        }
        self.parsed_bytes = text.len();
        self.rows.truncate(self.heading_rows + self.stable_rows);
        let mut tail = self
            .markdown
            .render_tail(text, assistant_width(width), MAX_VISUAL_ROWS);
        if tail.lines.last().is_some_and(|line| !line.spans.is_empty()) {
            tail.lines.push(Line::default());
        }
        let unstable = tail.lines.split_off(tail.stable_lines);
        self.rows.extend(assistant_body(
            tail.lines,
            &tail.links,
            width,
            MAX_VISUAL_ROWS - self.stable_rows,
        ));
        self.stable_rows = self.rows.len() - self.heading_rows;
        if self.stable_rows <= MAX_VISUAL_ROWS {
            self.rows.extend(assistant_body(
                unstable,
                &tail.links,
                width,
                MAX_VISUAL_ROWS - self.stable_rows,
            ));
        }
        &self.rows
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use serde_json::json;

    use super::{Row, StreamingReply};
    use crate::transcript::preview::render_assistant;

    fn same_rows(expected: &[Row], actual: &[Row]) {
        assert_eq!(expected.len(), actual.len());
        for (left, right) in expected.iter().zip(actual) {
            assert_eq!(left.line, right.line);
            assert_eq!(left.inset, right.inset);
            let destinations = |row: &Row| {
                row.links
                    .iter()
                    .map(|link| (link.columns.clone(), link.url.clone()))
                    .collect::<Vec<_>>()
            };
            assert_eq!(destinations(left), destinations(right));
        }
    }

    #[test]
    fn live_markup_matches_canonical_rows_as_structure_arrives() {
        let chunks = [
            "首段 **渐",
            "进显示**。\n\n",
            "| 项目 | 值 |\n| --- |",
            " --- |\n| 中文 | `a\\|b` |\n",
            "| 更多 | through |\n\n",
            "- 起始\n\n",
            "  - 一项\n\n",
            "- 完成\n\n",
            "```rust\nfn ",
            "greet() { println!(\"您好\"); }\n``",
            "`\n\n",
            "## Summary\n\n",
            "正文 <https://example.test/>\n\n",
            "下一个段落\n",
        ];
        for width in [60, 120, 12] {
            let mut text = String::new();
            let mut rendered = StreamingReply::default();
            for chunk in chunks {
                text.push_str(chunk);
                let expected = render_assistant(&text, false, true, width);
                same_rows(&expected, rendered.render(&text, false, width));
            }
        }
    }

    #[test]
    fn a_late_definition_and_a_repaired_document_reflow_their_suffixes() {
        let mut rendered = StreamingReply::default();
        let mut text = String::from("先完成上一段。\n\n");
        for chunk in [
            "[指南][guide] 先显示原文。\n\n",
            "下一段。\n\n",
            "[guide]: https://example.test/guide\n\n",
            "```markdown\n## 中文文档\n\n```rust\nfn main() {}\n```\n",
            "## 继续文档\n```\n\n",
            "回复尾部。",
        ] {
            text.push_str(chunk);
            same_rows(
                &render_assistant(&text, true, true, 60),
                rendered.render(&text, true, 60),
            );
        }
        assert_eq!("先完成上一段。\n\n".len(), rendered.markdown.stable_bytes());
    }

    #[test]
    fn stable_rows_are_retained_across_growth_and_relaid_out_on_resize() {
        let mut reply = StreamingReply::default();
        let mut text = "第一段保留。\n\n第二段也保留。\n\n最后一段开始。\n".to_owned();
        reply.render(&text, false, 120);
        assert!(reply.markdown.stable_bytes() >= "第一段保留。\n\n第二段也保留。\n\n".len());
        let original_storage = reply.rows[1].line.spans[0].content.as_ptr();
        text.push_str("More appended text.\n\n");
        let rows = reply.render(&text, false, 120);
        assert_eq!(original_storage, rows[1].line.spans[0].content.as_ptr());
        same_rows(
            &render_assistant(&text, true, true, 60),
            reply.render(&text, true, 60),
        );
    }

    #[test]
    fn the_total_line_limit_matches_canonical_layout() {
        let mut reply = StreamingReply::default();
        let mut text = "中文段落需要在窄屏上折行。".repeat(15) + "\n\n";
        for _ in 0..20 {
            text.push_str(&format!("{}\n\n", "中文。".repeat(110)));
            same_rows(
                &render_assistant(&text, false, true, 12),
                reply.render(&text, false, 12),
            );
        }
    }

    #[test]
    #[ignore = "layout timing for a deterministic local comparison"]
    fn stream_layout_profile() {
        let paragraph = |n| {
            format!(
                "Checkpoint {n:03} · {}\n\n",
                "逐项检查工作区结果，保留每项测试的结论，并核对后续变更与记录一致。".repeat(6)
            )
        };
        for width in [60, 120] {
            let mut text = String::from("首段。\n\n| 验证项 | 结果 |\n| --- | --- |\n| 中文 | 通过 |\n\n```rust\nfn preview() { }\n```\n\n");
            text.extend((0..42).map(paragraph));
            let mut reply = StreamingReply::default();
            let mut canonical = Vec::new();
            let mut incremental = Vec::new();
            let mut canonical_bytes = 0;
            let mut suffix_bytes = 0;
            reply.render(&text, false, width);
            for index in 42..58 {
                text.push_str(&paragraph(index));
                canonical_bytes += text.len();
                suffix_bytes += text.len() - reply.markdown.stable_bytes();
                let start = Instant::now();
                let expected = render_assistant(&text, false, true, width);
                canonical.push(start.elapsed());
                let start = Instant::now();
                let updated = reply.render(&text, false, width);
                incremental.push(start.elapsed());
                same_rows(&expected, updated);
            }
            let p95_us = |mut samples: Vec<Duration>| {
                samples.sort_unstable();
                samples[(samples.len() * 95).div_ceil(100) - 1].as_micros()
            };
            println!(
                "{}",
                json!({
                    "width": width,
                    "canonical_p95_us": p95_us(canonical),
                    "incremental_p95_us": p95_us(incremental),
                    "canonical_input_bytes": canonical_bytes,
                    "suffix_input_bytes": suffix_bytes,
                    "stable_source_bytes": reply.markdown.stable_bytes(),
                    "stable_wrapped_rows": reply.stable_rows,
                })
            );
        }
    }
}
