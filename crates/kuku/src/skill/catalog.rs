use super::registry::SkillRegistry;

pub fn render_skill_catalog(registry: &SkillRegistry) -> Option<String> {
    if registry.is_empty() {
        return None;
    }

    let mut entries = String::new();
    for def in registry.definitions() {
        let path = def
            .source_path
            .as_deref()
            .unwrap_or(def.source.as_str());
        entries.push_str(&format!(
            "- {} — {} ({})\n",
            def.name, def.description, path,
        ));
    }

    Some(format!(
        "<kuku_skill_catalog>\nAvailable skills:\n{entries}</kuku_skill_catalog>",
        entries = entries,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_renders_skills() {
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
        let catalog = render_skill_catalog(&registry).expect("should render");
        assert!(catalog.contains("<kuku_skill_catalog"));
        assert!(catalog.contains("Available skills:"));
        assert!(catalog.contains("- tdd — Write tests first"));
        let expected_path = std::path::Path::new(".kuku").join("skills").join("tdd");
        let expected_path_str = expected_path.to_string_lossy().into_owned();
        assert!(catalog.contains(&expected_path_str));
        assert!(
            !catalog.contains("Instructions"),
            "catalog must NOT include full instructions"
        );
        assert!(!catalog.contains("<skill"), "no XML skill tags");
    }

    #[test]
    fn catalog_is_none_for_empty_registry() {
        let registry = crate::skill::registry::SkillRegistry::builder().build();
        assert!(render_skill_catalog(&registry).is_none());
    }
}
