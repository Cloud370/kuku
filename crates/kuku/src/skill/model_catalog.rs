use std::fmt::Write as _;

use crate::context::{CanonicalMessage, MessageBlock, estimate_text_tokens};

use super::registry::{SkillRegistry, detect_skill_changes};

pub(crate) const DESCRIPTION_BUDGET: u32 = 8_192;
const FULL: &str = "<kuku_skill_catalog>";
const CHANGES: &str = "<kuku_skill_changes>";
const ANCHOR: &str = "<!-- skill-catalog: ";

pub(crate) enum CatalogAnchor<'a> {
    Full(&'a str),
    Delta { from: &'a str, to: &'a str },
}

pub(crate) fn anchor(message: &CanonicalMessage) -> Option<CatalogAnchor<'_>> {
    let [MessageBlock::Text(text)] = message.blocks.as_slice() else {
        return None;
    };
    let mut lines = text.lines();
    let kind = lines.next()?;
    let fields = lines.next()?.strip_prefix(ANCHOR)?.strip_suffix(" -->")?;
    let mut fields = fields.split_whitespace();
    let id = fields.next().filter(|id| valid_id(id))?;
    match (kind, fields.next(), fields.next()) {
        (FULL, None, None) => Some(CatalogAnchor::Full(id)),
        (CHANGES, Some(from), None) if valid_id(from) => {
            Some(CatalogAnchor::Delta { from, to: id })
        }
        _ => None,
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn full(registry: &SkillRegistry, id: &str) -> CanonicalMessage {
    let mut text = format!(
        "{FULL}\n{ANCHOR}{id} -->\nComplete Skill catalog: {} names. This list replaces earlier catalog entries.\n",
        registry.len(),
    );
    let rows: Vec<_> = registry
        .listing_names()
        .iter()
        .map(|name| {
            (
                name.to_owned(),
                registry.get(name).map(|def| def.description.as_str()),
            )
        })
        .collect();
    append_rows(&mut text, rows);
    text.push_str("</kuku_skill_catalog>");
    CanonicalMessage::user_text(text)
}

pub(crate) fn changes(
    old: &SkillRegistry,
    new: &SkillRegistry,
    from: &str,
    to: &str,
) -> Option<CanonicalMessage> {
    let changes = detect_skill_changes(old, new)?;
    let mut text =
        format!("{CHANGES}\n{ANCHOR}{to} {from} -->\nSkill catalog changed for this Turn:\n");
    let mut rows = Vec::new();
    for name in changes.added {
        let description = new.get(&name).map(|def| def.description.as_str());
        rows.push((format!("Added: {name}"), description));
    }
    for name in changes.updated {
        let description =
            new.get(&name)
                .map(|def| def.description.as_str())
                .filter(|description| {
                    old.get(&name)
                        .is_none_or(|def| def.description != *description)
                });
        rows.push((format!("Updated: {name} (new revision)"), description));
    }
    for name in changes.removed {
        rows.push((format!("Removed: {name}"), None));
    }
    append_rows(&mut text, rows);
    text.push_str("</kuku_skill_changes>");
    Some(CanonicalMessage::user_text(text))
}

fn append_rows(text: &mut String, rows: Vec<(String, Option<&str>)>) {
    let rows: Vec<_> = rows
        .into_iter()
        .map(|(name, description)| {
            let description = description.map_or_else(String::new, |text| {
                text.split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
            });
            let cost = estimate_text_tokens(&description);
            (name, description, cost)
        })
        .collect();
    let allowance = description_allowance(rows.iter().map(|(_, _, cost)| *cost));
    let shortened = rows.iter().filter(|(_, _, cost)| *cost > allowance).count();
    if shortened > 0 {
        let _ = writeln!(
            text,
            "{shortened} descriptions were shortened or omitted to fit the description budget. Names are complete."
        );
    }
    for (name, description, cost) in rows {
        let description = if cost <= allowance {
            description
        } else {
            shorten(&description, allowance)
        };
        if description.is_empty() {
            let _ = writeln!(text, "{name}");
        } else {
            let _ = writeln!(text, "{name} — {description}");
        }
    }
}

fn description_allowance(costs: impl Iterator<Item = u32>) -> u32 {
    let mut costs: Vec<_> = costs.filter(|cost| *cost != 0).collect();
    if costs
        .iter()
        .fold(0u32, |total, cost| total.saturating_add(*cost))
        <= DESCRIPTION_BUDGET
    {
        return DESCRIPTION_BUDGET;
    }
    costs.sort_unstable();
    let mut remaining = DESCRIPTION_BUDGET;
    for (index, cost) in costs.iter().enumerate() {
        let count = u32::try_from(costs.len() - index).unwrap_or(u32::MAX);
        let share = remaining / count;
        if *cost > share {
            return share;
        }
        remaining -= cost;
    }
    DESCRIPTION_BUDGET
}

fn shorten(text: &str, limit: u32) -> String {
    if limit == 0 {
        return String::new();
    }
    let max_units = limit.saturating_sub(1).saturating_mul(4);
    let mut end = 0;
    let mut sentence = 0;
    let mut units = 0u32;
    for (index, ch) in text.char_indices() {
        units += if ch.is_ascii() { 1 } else { 4 };
        if units > max_units {
            break;
        }
        end = index + ch.len_utf8();
        if matches!(ch, '.' | '!' | '?' | '。' | '！' | '？') {
            sentence = end;
        }
    }
    if sentence > 0 && sentence >= end / 2 {
        end = sentence;
    }
    format!("{}…", text[..end].trim_end())
}

#[cfg(test)]
mod tests {
    use super::{DESCRIPTION_BUDGET, append_rows, estimate_text_tokens, shorten};

    #[test]
    fn one_hundred_full_descriptions_fit_and_larger_catalog_is_marked() {
        let description = "短句帮助索引。".repeat(10);
        let render = |count: usize| {
            let mut text = String::new();
            append_rows(
                &mut text,
                (0..count)
                    .map(|i| (format!("skill-{i:04}"), Some(description.as_str())))
                    .collect(),
            );
            text
        };
        let small = render(100);
        assert_eq!(small.matches(&description).count(), 100);
        assert!(!small.contains("shortened"));
        let large = render(1202);
        assert!(large.contains("1202 descriptions were shortened"));
        assert_eq!(
            large
                .lines()
                .filter(|line| line.starts_with("skill-"))
                .count(),
            1202
        );
        let model_view_cost = large
            .lines()
            .filter_map(|line| line.split_once(" — "))
            .map(|(_, description)| estimate_text_tokens(description))
            .sum::<u32>();
        assert!(model_view_cost <= DESCRIPTION_BUDGET);
        assert!(estimate_text_tokens(&shorten("小短句。x你好🙂非常长的尾巴", 8)) <= 8);
    }
}
