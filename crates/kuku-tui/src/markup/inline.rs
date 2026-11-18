use super::Markdown;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Position {
    Superscript,
    Subscript,
}

impl Position {
    pub(super) fn convert(self, text: &str) -> String {
        let replacements = match self {
            Self::Superscript => "⁰¹²³⁴⁵⁶⁷⁸⁹⁺⁻⁼⁽⁾",
            Self::Subscript => "₀₁₂₃₄₅₆₇₈₉₊₋₌₍₎",
        };
        text.chars()
            .map(|ch| {
                "0123456789+-=()"
                    .find(ch)
                    .and_then(|i| replacements.chars().nth(i))
                    .unwrap_or(ch)
            })
            .collect()
    }
}

impl Markdown {
    pub(super) fn inline_html(&mut self, tag: &str) {
        match tag.to_ascii_lowercase().as_str() {
            "<sup>" => self.script_position = Some(Position::Superscript),
            "<sub>" => self.script_position = Some(Position::Subscript),
            "</sup>" if self.script_position == Some(Position::Superscript) => {
                self.script_position = None
            }
            "</sub>" if self.script_position == Some(Position::Subscript) => {
                self.script_position = None
            }
            "<br>" | "<br/>" | "<br />" => self.new_line(),
            _ => self.text(tag),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::markup::render_markdown;

    #[test]
    fn inline_html_preserves_literal_code_and_breaks_lines() {
        let lines = render_markdown(
            "x<sup>2</sup> H<sub>2</sub>O<br>下一行\n\n`<sup>2</sup>`",
            40,
            20,
        );
        let text: Vec<_> = lines.iter().map(ToString::to_string).collect();
        assert_eq!(&text[..3], &["x² H₂O", "下一行", ""]);
        assert!(text.iter().any(|line| line == "<sup>2</sup>"));
    }

    #[test]
    fn wrapped_quoted_task_preserves_its_continuation_gutter() {
        let lines = render_markdown("> - [x] abcdefghijklmnop", 12, 20);
        assert!(lines[0].to_string().starts_with("│ • [x] "));
        assert!(lines[1].to_string().starts_with("│   "));
    }
}
