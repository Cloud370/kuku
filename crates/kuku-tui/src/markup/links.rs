use std::ops::Range;
use std::sync::Arc;

use ratatui::style::{Color, Modifier};
use ratatui::text::Line;

use crate::theme::ACCENT;

#[derive(Clone, Debug)]
pub(crate) struct LinkDestination {
    pub(crate) columns: Range<usize>,
    pub(crate) url: Arc<str>,
}

#[derive(Default)]
pub(crate) struct LinkLayout(Vec<Arc<str>>);

impl LinkLayout {
    // Ratatui wraps and rearranges styled spans in tables. Carry the destination index
    // through layout on underlined spans, then restore the theme before display.
    pub(super) fn register(&mut self, url: &str) -> Color {
        let id = self.0.len();
        self.0.push(Arc::from(url));
        Color::Rgb((id >> 16) as u8, (id >> 8) as u8, id as u8)
    }

    pub(crate) fn resolve(&self, line: &mut Line<'_>) -> Vec<LinkDestination> {
        let mut links = Vec::new();
        let mut column = 0;
        for span in &mut line.spans {
            let end = column + span.width();
            if span.style.add_modifier.contains(Modifier::UNDERLINED) {
                if let Some(Color::Rgb(r, g, b)) = span.style.fg {
                    let id = usize::from(r) << 16 | usize::from(g) << 8 | usize::from(b);
                    if let Some(url) = self.0.get(id) {
                        links.push(LinkDestination {
                            columns: column..end,
                            url: Arc::clone(url),
                        });
                        span.style.fg = Some(ACCENT);
                    }
                }
            }
            column = end;
        }
        links
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::markup::{linked_markdown, wrap_bounded};

    #[test]
    fn links_use_wrapped_cell_positions_and_preserve_duplicate_labels() {
        let source = "[指南](https://a.test/) [指南](https://b.test/?v=2#top)";
        let (lines, layout) = linked_markdown(source, 5, 20);
        let mut rows = wrap_bounded(lines, 5, 20);
        let first = layout.resolve(&mut rows[0]);
        let second = layout.resolve(&mut rows[1]);
        assert_eq!(first[0].columns, 0..4);
        assert_eq!(second[0].columns, 0..4);
        assert_eq!(first[0].url.as_ref(), "https://a.test/");
        assert_eq!(second[0].url.as_ref(), "https://b.test/?v=2#top");
    }

    #[test]
    fn links_in_preview_tables_keep_destinations_and_code_stays_literal() {
        let source = "````md\n| A | B |\n| --- | --- |\n| [x](https://a.test/) | [x](https://b.test/) |\n````\n\n`[x](https://c.test/)` [other](file:///tmp/path)";
        let (mut rows, layout) = linked_markdown(source, 42, 50);
        let found: BTreeSet<_> = rows
            .iter_mut()
            .flat_map(|line| layout.resolve(line))
            .map(|link| link.url.to_string())
            .collect();
        assert_eq!(
            found,
            BTreeSet::from(["https://a.test/".to_owned(), "https://b.test/".to_owned()])
        );
    }
}
