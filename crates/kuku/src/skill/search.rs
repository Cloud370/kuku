use serde_json::{json, Value};

use crate::event::StoredEvent;

use super::definition::SkillDefinition;
use super::registry::SkillRegistry;
use super::session::loaded_skill_names;

pub(crate) fn list_skills_result(
    registry: &SkillRegistry,
    events: &[StoredEvent],
    args: &Value,
) -> Value {
    let offset = pagination_offset(args);
    let limit = pagination_limit(args, 20, 50);
    let loaded = loaded_skill_names(events)
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let definitions = registry.definitions();
    let total = definitions.len();
    let skills = definitions
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|definition| skill_item(definition, loaded.contains(definition.name.as_str())))
        .collect::<Vec<_>>();

    json!({
        "offset": offset,
        "limit": limit,
        "total": total,
        "skills": skills,
    })
}

pub(crate) fn search_skills_result(
    registry: &SkillRegistry,
    events: &[StoredEvent],
    args: &Value,
) -> Value {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let offset = pagination_offset(args);
    let limit = pagination_limit(args, 10, 25);
    let loaded = loaded_skill_names(events)
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();

    let mut matches = registry
        .definitions()
        .into_iter()
        .filter_map(|definition| {
            let score = skill_search_score(definition, query)?;
            Some((score, definition))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|(left_score, left_def), (right_score, right_def)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_def.name.cmp(&right_def.name))
    });

    let total = matches.len();
    let skills = matches
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(_, definition)| skill_item(definition, loaded.contains(definition.name.as_str())))
        .collect::<Vec<_>>();

    json!({
        "offset": offset,
        "limit": limit,
        "total": total,
        "skills": skills,
    })
}

fn skill_item(definition: &SkillDefinition, loaded: bool) -> Value {
    json!({
        "name": definition.name,
        "description": definition.description,
        "source": definition.source.as_str(),
        "loaded": loaded,
    })
}

fn skill_search_score(
    definition: &SkillDefinition,
    query: &str,
) -> Option<(u32, u32, u32, u32, u32)> {
    let normalized_query = normalize(query);
    if normalized_query.is_empty() {
        return None;
    }
    let terms = normalized_query
        .split_whitespace()
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    if terms.is_empty() {
        return None;
    }

    let title = skill_title(definition);
    let headings = skill_headings(definition);
    let body = skill_body(definition);

    let name_score = field_score(&normalize(&definition.name), &normalized_query, &terms);
    let description_score = field_score(
        &normalize(&definition.description),
        &normalized_query,
        &terms,
    );
    let title_score = field_score(&normalize(&title), &normalized_query, &terms);
    let heading_score = field_score(&normalize(&headings), &normalized_query, &terms);
    let body_score = field_score(&normalize(&body), &normalized_query, &terms);

    let total =
        name_score * 32 + description_score * 16 + title_score * 8 + heading_score * 4 + body_score;
    (total > 0).then_some((
        total,
        name_score,
        description_score + title_score,
        heading_score,
        body_score,
    ))
}

fn field_score(field: &str, query: &str, terms: &[&str]) -> u32 {
    if field.is_empty() {
        return 0;
    }
    let mut score = 0;
    if field.contains(query) {
        score += 8;
    }
    for term in terms {
        if field.contains(term) {
            score += 1;
        }
    }
    score
}

fn skill_title(definition: &SkillDefinition) -> String {
    if let Some(title) = definition.metadata.get("title").and_then(Value::as_str) {
        return title.to_string();
    }
    definition
        .instructions
        .lines()
        .find_map(|line| line.strip_prefix("# ").map(str::trim))
        .unwrap_or_default()
        .to_string()
}

fn skill_headings(definition: &SkillDefinition) -> String {
    definition
        .instructions
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix('#')
                .map(|rest| rest.trim_start_matches('#').trim())
                .filter(|heading| !heading.is_empty())
                .map(str::to_string)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn skill_body(definition: &SkillDefinition) -> String {
    definition
        .instructions
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize(value: &str) -> String {
    value.chars().flat_map(|ch| ch.to_lowercase()).collect()
}

fn pagination_offset(args: &Value) -> usize {
    args.get("offset")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0)
}

fn pagination_limit(args: &Value, default: usize, max: usize) -> usize {
    args.get("limit")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
        .map(|value| value.min(max))
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::event::{EventPayload, StoredEvent};
    use crate::skill::definition::{SkillDefinition, SkillSource};
    use crate::skill::registry::SkillRegistry;

    fn skill(
        name: &str,
        description: &str,
        instructions: &str,
        metadata: serde_json::Value,
    ) -> SkillDefinition {
        let mut definition = SkillDefinition {
            name: name.to_string(),
            description: description.to_string(),
            instructions: instructions.to_string(),
            source: SkillSource::Project,
            hash: String::new(),
            source_path: Some(format!("/skills/{name}")),
            allowed_tools: None,
            disallowed_tools: None,
            max_turns: None,
            model: None,
            license: None,
            compatibility: None,
            metadata,
        };
        definition.hash = definition.compute_hash();
        definition
    }

    fn registry() -> SkillRegistry {
        SkillRegistry::builder()
            .with_definition(skill(
                "alpha",
                "Alpha workflow",
                "# Alpha\n\nGeneral setup.",
                json!({"title": "Alpha Title"}),
            ))
            .with_definition(skill(
                "beta-review",
                "Ranking review pull requests",
                "# Beta\n\n## Ranking\nFocus on ranking results.",
                json!({"title": "Beta Review"}),
            ))
            .with_definition(skill(
                "gamma",
                "General helper",
                "# Search Wizard\n\n## Ranking\nbody mentions ranking twice. ranking",
                json!({}),
            ))
            .build()
    }

    fn events() -> Vec<StoredEvent> {
        vec![
            StoredEvent {
                id: 1,
                payload: EventPayload::ContextSkills {
                    turn: 1,
                    ts: "t1".to_string(),
                    registry: registry(),
                    bootstrap_loaded: vec![],
                },
            },
            StoredEvent {
                id: 2,
                payload: EventPayload::ToolCall {
                    turn: 1,
                    ts: "t2".to_string(),
                    tool_call_id: "tool_beta".to_string(),
                    request_id: "req_1".to_string(),
                    index: 0,
                    tool: "use_skill".to_string(),
                    args: json!({ "skill_name": "beta-review" }),
                },
            },
            StoredEvent {
                id: 3,
                payload: EventPayload::ToolResult {
                    turn: 1,
                    ts: "t3".to_string(),
                    tool_call_id: "tool_beta".to_string(),
                    status: "ok".to_string(),
                    summary: "loaded skill: beta-review".to_string(),
                    model_content: String::new(),
                    truncated: false,
                    structured: None,
                },
            },
        ]
    }

    #[test]
    fn list_skills_pages_snapshot_and_marks_loaded() {
        let result =
            super::list_skills_result(&registry(), &events(), &json!({ "offset": 1, "limit": 1 }));

        assert_eq!(result["offset"], 1);
        assert_eq!(result["limit"], 1);
        assert_eq!(result["total"], 3);
        assert_eq!(result["skills"].as_array().unwrap().len(), 1);
        assert_eq!(result["skills"][0]["name"], "beta-review");
        assert_eq!(result["skills"][0]["loaded"], true);
        assert_eq!(result["skills"][0]["source"], "project");
    }

    #[test]
    fn search_skills_ranks_name_then_description_then_body() {
        let result = super::search_skills_result(
            &registry(),
            &events(),
            &json!({ "query": "ranking", "limit": 3 }),
        );
        let items = result["skills"].as_array().unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["name"], "beta-review");
        assert_eq!(items[0]["loaded"], true);
        assert_eq!(items[1]["name"], "gamma");
    }

    #[test]
    fn search_skills_is_case_insensitive_and_matches_metadata_title() {
        let result = super::search_skills_result(
            &registry(),
            &events(),
            &json!({ "query": "alpha title", "limit": 10 }),
        );
        let items = result["skills"].as_array().unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["name"], "alpha");
    }

    #[test]
    fn list_skills_applies_default_and_max_limit() {
        let default_result = super::list_skills_result(&registry(), &events(), &json!({}));
        let capped_result =
            super::list_skills_result(&registry(), &events(), &json!({ "limit": 999 }));

        assert_eq!(default_result["limit"], 20);
        assert_eq!(capped_result["limit"], 50);
    }

    #[test]
    fn search_skills_applies_default_and_max_limit() {
        let default_result =
            super::search_skills_result(&registry(), &events(), &json!({ "query": "ranking" }));
        let capped_result = super::search_skills_result(
            &registry(),
            &events(),
            &json!({ "query": "ranking", "limit": 999 }),
        );

        assert_eq!(default_result["limit"], 10);
        assert_eq!(capped_result["limit"], 25);
    }
}
