use super::registry::SkillRegistry;

pub fn render_skill_catalog(registry: &SkillRegistry) -> Option<String> {
    if registry.is_empty() {
        return None;
    }

    let mut entries = String::new();
    for def in registry.definitions() {
        entries.push_str(&format!(
            "  <skill name=\"{name}\" source=\"{source}\" hash=\"{hash}\">\n    <description>{description}</description>\n  </skill>\n",
            name = def.name,
            source = def.source.as_str(),
            hash = def.hash,
            description = def.description,
        ));
    }

    Some(format!(
        "<kuku_skill_catalog version=\"{version}\">\n{entries}</kuku_skill_catalog>",
        version = registry.hash(),
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
            .load_kuku_project_skills(dir.path())
            .unwrap()
            .build();
        let catalog = render_skill_catalog(&registry).expect("should render");
        assert!(catalog.contains("<kuku_skill_catalog"));
        assert!(catalog.contains("name=\"tdd\""));
        assert!(catalog.contains("source=\"kuku:project\""));
        assert!(catalog.contains("Write tests first"));
        assert!(
            !catalog.contains("Instructions"),
            "catalog must NOT include full instructions"
        );
    }

    #[test]
    fn catalog_is_none_for_empty_registry() {
        let registry = crate::skill::registry::SkillRegistry::builder().build();
        assert!(render_skill_catalog(&registry).is_none());
    }
}
