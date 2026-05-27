use serde_yaml::Mapping;

pub(crate) fn split_yaml_frontmatter(content: &str) -> (Option<Mapping>, &str) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (None, content);
    }
    let after_first = &trimmed[3..];
    let Some(end) = after_first.find("\n---") else {
        return (None, content);
    };
    let yaml_str = &after_first[..end];
    let body = &after_first[end + 4..];
    let mapping = serde_yaml::from_str::<Mapping>(yaml_str).ok();
    (mapping, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_frontmatter() {
        let input = "---\nname: test\n---\nbody here";
        let (mapping, body) = split_yaml_frontmatter(input);
        assert!(mapping.is_some());
        assert_eq!(body.trim(), "body here");
    }

    #[test]
    fn no_frontmatter() {
        let input = "just plain text";
        let (mapping, body) = split_yaml_frontmatter(input);
        assert!(mapping.is_none());
        assert_eq!(body, "just plain text");
    }

    #[test]
    fn malformed_yaml_returns_none() {
        let input = "---\n:::invalid:::\n---\nbody";
        let (mapping, body) = split_yaml_frontmatter(input);
        assert!(mapping.is_none());
        assert_eq!(body.trim(), "body");
    }

    #[test]
    fn unclosed_frontmatter() {
        let input = "---\nname: test\nno closing delimiter";
        let (mapping, body) = split_yaml_frontmatter(input);
        assert!(mapping.is_none());
        assert_eq!(body, input);
    }

    #[test]
    fn leading_whitespace() {
        let input = "  \n---\nname: test\n---\nbody";
        let (mapping, _) = split_yaml_frontmatter(input);
        assert!(mapping.is_some());
    }
}
