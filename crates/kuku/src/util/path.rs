pub(crate) fn normalize_path_sep(path: &str) -> String {
    path.replace('\\', "/")
}

pub(crate) fn is_blocked_relative_path(path: &str) -> bool {
    let normalized = normalize_path_sep(path);
    normalized
        .split('/')
        .any(|part| part == ".git" || part == ".ssh")
        || normalized
            .rsplit('/')
            .next()
            .is_some_and(is_sensitive_file_name)
}

pub(crate) fn is_sensitive_file_name(name: &str) -> bool {
    matches!(
        name,
        ".env"
            | "credentials.json"
            | "credentials.toml"
            | "id_rsa"
            | "id_dsa"
            | "id_ecdsa"
            | "id_ed25519"
    ) || name.starts_with(".env.")
        || name.ends_with(".pem")
        || name.ends_with(".key")
        || name.ends_with(".p12")
        || name.ends_with(".pfx")
}
