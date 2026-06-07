use super::registry::{SkillChanges, SkillRegistry};

const MAX_SKILLS_BLOCK_BYTES: usize = 2_048;
const HINT_LIST_SKILLS: &str = "Use list_skills to browse available skills.";
const HINT_SEARCH_SKILLS: &str = "Use search_skills to find skills by task or workflow.";
const CHANGE_LINE_BUDGET: usize = 384;
const LOADED_LINE_BUDGET: usize = 384;
const PREVIEW_LINE_BUDGET: usize = 320;

pub fn render_skill_catalog(
    registry: &SkillRegistry,
    loaded_skill_names: &[String],
    changes: Option<&SkillChanges>,
) -> Option<String> {
    let change_line = changes.and_then(render_skill_changes);
    let loaded_line = render_loaded_skills(loaded_skill_names);
    let total_line = format!("Available skills: {} total", registry.len());
    let hint_lines = [HINT_LIST_SKILLS.to_string(), HINT_SEARCH_SKILLS.to_string()];

    let mut header_lines = Vec::new();
    if let Some(change_line) = change_line {
        header_lines.push(change_line);
    }
    header_lines.push(loaded_line);
    header_lines.push(total_line);

    let preview_candidates = registry
        .definitions()
        .into_iter()
        .map(|definition| {
            render_preview_line(definition.name.as_str(), definition.description.as_str())
        })
        .collect::<Vec<_>>();
    let block = pack_skill_block(&header_lines, &preview_candidates, &hint_lines);

    debug_assert!(block.len() <= MAX_SKILLS_BLOCK_BYTES);
    Some(block)
}

fn pack_skill_block(
    header_lines: &[String],
    preview_candidates: &[String],
    hint_lines: &[String; 2],
) -> String {
    let mut preview_lines = Vec::new();
    let mut best_block = assemble_skill_block(
        header_lines,
        &preview_lines,
        overflow_line(preview_candidates.len()).as_deref(),
        hint_lines,
    );

    for preview_candidate in preview_candidates {
        let mut candidate_preview_lines = preview_lines.clone();
        candidate_preview_lines.push(preview_candidate.clone());
        let hidden_count = preview_candidates
            .len()
            .saturating_sub(candidate_preview_lines.len());
        let candidate_block = assemble_skill_block(
            header_lines,
            &candidate_preview_lines,
            overflow_line(hidden_count).as_deref(),
            hint_lines,
        );

        if candidate_block.len() > MAX_SKILLS_BLOCK_BYTES {
            break;
        }

        preview_lines = candidate_preview_lines;
        best_block = candidate_block;
    }

    best_block
}

fn assemble_skill_block(
    header_lines: &[String],
    preview_lines: &[String],
    overflow_line: Option<&str>,
    hint_lines: &[String; 2],
) -> String {
    let mut lines = Vec::new();
    lines.push("<kuku_skills>".to_string());
    lines.extend(header_lines.iter().cloned());
    lines.extend(preview_lines.iter().cloned());
    if let Some(overflow_line) = overflow_line {
        lines.push(overflow_line.to_string());
    }
    lines.extend(hint_lines.iter().cloned());
    lines.push("</kuku_skills>".to_string());
    lines.join("\n")
}

fn overflow_line(hidden_count: usize) -> Option<String> {
    (hidden_count > 0).then(|| format!("... +{hidden_count} more"))
}

fn render_skill_changes(changes: &SkillChanges) -> Option<String> {
    let mut tokens = Vec::new();
    tokens.extend(changes.added.iter().map(|name| format!("+{name}")));
    tokens.extend(changes.updated.iter().map(|name| format!("~{name}")));
    tokens.extend(changes.removed.iter().map(|name| format!("-{name}")));

    render_compact_name_line("Changed: ", &tokens, CHANGE_LINE_BUDGET)
}

fn render_loaded_skills(loaded_skill_names: &[String]) -> String {
    render_compact_name_line("Loaded: ", loaded_skill_names, LOADED_LINE_BUDGET)
        .unwrap_or_else(|| "Loaded: none".to_string())
}

fn render_compact_name_line<T>(prefix: &str, names: &[T], max_bytes: usize) -> Option<String>
where
    T: AsRef<str>,
{
    if names.is_empty() {
        return None;
    }

    let mut selected: Vec<String> = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let name = name.as_ref();
        let remaining = names.len().saturating_sub(index + 1);
        let mut candidate_names = selected.clone();
        candidate_names.push(name.to_string());
        let candidate = if remaining == 0 {
            format!("{prefix}{}", candidate_names.join(", "))
        } else {
            format!(
                "{prefix}{}... +{remaining} more",
                candidate_names.join(", ")
            )
        };
        if candidate.len() <= max_bytes {
            selected.push(name.to_string());
            continue;
        }

        return Some(if selected.is_empty() {
            format!("{prefix}... +{} more", names.len())
        } else {
            format!(
                "{prefix}{}, ... +{} more",
                selected.join(", "),
                names.len() - selected.len()
            )
        });
    }

    Some(format!("{prefix}{}", selected.join(", ")))
}

fn render_preview_line(name: &str, description: &str) -> String {
    truncate_utf8_bytes(&format!("{name} - {description}"), PREVIEW_LINE_BUDGET)
}

fn truncate_utf8_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }

    let suffix = "...";
    let target = max_bytes.saturating_sub(suffix.len());
    let mut end = 0;
    for (index, _) in text.char_indices() {
        if index > target {
            break;
        }
        end = index;
    }
    if end == 0 {
        return suffix.to_string();
    }
    format!("{}{}", &text[..end], suffix)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::registry::SkillChanges;

    #[test]
    fn catalog_renders_budgeted_skill_block_without_paths() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".kuku").join("skills").join("tdd");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: tdd\ndescription: Write tests first\n---\n\nInstructions.\n",
        )
        .unwrap();

        let registry = crate::skill::registry::SkillRegistry::builder()
            .load_from_dir(
                &dir.path().join(".kuku").join("skills"),
                crate::skill::definition::SkillSource::Project,
            )
            .unwrap()
            .build();
        let catalog = render_skill_catalog(
            &registry,
            &["review".to_string(), "tdd".to_string()],
            Some(&SkillChanges {
                added: vec!["review".to_string()],
                updated: vec!["tdd".to_string()],
                removed: vec!["legacy".to_string()],
            }),
        )
        .expect("should render");
        assert!(catalog.contains("<kuku_skills>"));
        assert!(catalog.contains("Changed:"));
        assert!(catalog.contains("Loaded: review, tdd"));
        assert!(catalog.contains("Available skills: 1 total"));
        assert!(catalog.contains("tdd - Write tests first"));
        assert!(catalog.contains("Use list_skills to browse available skills."));
        assert!(catalog.contains("Use search_skills to find skills by task or workflow."));
        let expected_path = std::path::Path::new(".kuku").join("skills").join("tdd");
        let expected_path_str = expected_path.to_string_lossy().into_owned();
        assert!(!catalog.contains(&expected_path_str));
        assert!(
            !catalog.contains("Instructions"),
            "catalog must NOT include full instructions"
        );
        assert!(catalog.len() <= 2048);
    }

    #[test]
    fn catalog_reserves_space_for_required_hints_when_previews_overflow() {
        let mut builder = crate::skill::registry::SkillRegistry::builder();
        for index in 0..10 {
            builder = builder.with_definition(skill_definition(
                &format!("skill-{index}"),
                &"A".repeat(600),
            ));
        }
        let registry = builder.build();

        let catalog = render_skill_catalog(&registry, &[], None).expect("should render");

        assert!(catalog.starts_with("<kuku_skills>\n"));
        assert!(catalog.contains("Available skills: 10 total"));
        assert!(catalog.contains("... +"));
        assert!(catalog.contains("Use list_skills to browse available skills."));
        assert!(catalog.contains("Use search_skills to find skills by task or workflow."));
        assert!(catalog.len() <= 2048);
    }

    #[test]
    fn catalog_renders_empty_registry_with_zero_count_and_hints() {
        let registry = crate::skill::registry::SkillRegistry::builder().build();
        let catalog = render_skill_catalog(&registry, &["tdd".to_string()], None)
            .expect("empty registry should still render");

        assert!(catalog.contains("<kuku_skills>"));
        assert!(catalog.contains("Loaded: tdd"));
        assert!(catalog.contains("Available skills: 0 total"));
        assert!(catalog.contains("Use list_skills to browse available skills."));
        assert!(catalog.contains("Use search_skills to find skills by task or workflow."));
        assert!(catalog.len() <= 2048);
    }

    #[test]
    fn truncate_utf8_bytes_preserves_character_boundaries() {
        let text = format!("skill - {}", "界".repeat(200));

        let truncated = truncate_utf8_bytes(&text, 25);

        assert!(truncated.as_bytes().len() <= 25);
        assert!(truncated.ends_with("..."));
        assert!(!truncated.contains(' '));
        let rebuilt = truncated[..truncated.len() - 3].chars().collect::<String>();
        assert_eq!(rebuilt.chars().last(), Some('界'));
    }

    #[test]
    fn packed_skill_block_uses_final_overflow_count_when_rejecting_preview() {
        let header_lines = vec![
            "Changed: ".to_string() + &"c".repeat(255),
            "Loaded: none".to_string(),
            "Available skills: 15 total".to_string(),
        ];
        let preview_candidates = vec!["p".repeat(320); 15];
        let hint_lines = [HINT_LIST_SKILLS.to_string(), HINT_SEARCH_SKILLS.to_string()];

        let block = pack_skill_block(&header_lines, &preview_candidates, &hint_lines);

        assert!(block.as_bytes().len() <= MAX_SKILLS_BLOCK_BYTES);
        assert!(block.contains("... +11 more"));
        assert!(!block.contains("... +9 more"));
    }

    #[test]
    fn packed_skill_block_respects_utf8_byte_budget() {
        let header_lines = vec![
            "Loaded: none".to_string(),
            "Available skills: 12 total".to_string(),
        ];
        let preview_candidates = vec![render_preview_line("skill", &"界".repeat(400)); 12];
        let hint_lines = [HINT_LIST_SKILLS.to_string(), HINT_SEARCH_SKILLS.to_string()];

        let block = pack_skill_block(&header_lines, &preview_candidates, &hint_lines);

        assert!(block.as_bytes().len() <= MAX_SKILLS_BLOCK_BYTES);
        assert!(block.contains("... +"));
        assert!(!block.contains('\u{fffd}'));
    }

    #[test]
    fn catalog_stays_within_budget_at_overflow_digit_boundaries() {
        for total_skills in [10_usize, 11, 99, 100, 101] {
            let mut builder = crate::skill::registry::SkillRegistry::builder();
            for index in 0..total_skills {
                builder = builder.with_definition(skill_definition(
                    &format!("skill-{index:03}"),
                    &"界".repeat(400),
                ));
            }
            let registry = builder.build();
            let catalog = render_skill_catalog(&registry, &[], None).expect("should render");

            assert!(
                catalog.as_bytes().len() <= MAX_SKILLS_BLOCK_BYTES,
                "catalog exceeded byte budget for total_skills={total_skills}, bytes={}",
                catalog.as_bytes().len()
            );
        }
    }

    fn skill_definition(
        name: &str,
        description: &str,
    ) -> crate::skill::definition::SkillDefinition {
        let mut definition = crate::skill::definition::SkillDefinition {
            name: name.to_string(),
            description: description.to_string(),
            instructions: format!("{name} instructions"),
            source: crate::skill::definition::SkillSource::Project,
            hash: String::new(),
            source_path: Some(format!("/skills/{name}")),
            allowed_tools: None,
            disallowed_tools: None,
            max_turns: None,
            model: None,
            license: None,
            compatibility: None,
            metadata: serde_json::Value::Null,
        };
        definition.hash = definition.compute_hash();
        definition
    }
}
