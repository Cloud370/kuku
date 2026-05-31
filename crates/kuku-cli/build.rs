fn main() {
    let version = env!("CARGO_PKG_VERSION");
    let hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let full = if hash.is_empty() {
        version.to_string()
    } else {
        format!("{version} ({hash})")
    };
    println!("cargo:rustc-env=KUKU_VERSION={full}");
}
