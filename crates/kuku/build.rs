use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "src/skill/bundle/format.rs"]
mod bundle_format;
#[path = "build/docs.rs"]
mod docs;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build/docs.rs");
    println!("cargo:rerun-if-changed=src/skill/bundle/format.rs");
    // A translation check can mount a different Git tree at the same docs path.
    println!("cargo:rerun-if-env-changed=KUKU_DOCS_INPUT_TREE");
    let git_revision = Path::new("../../.git")
        .exists()
        .then(|| git(&["rev-parse", "HEAD"]))
        .flatten();
    let revision = source_revision(git_revision.as_deref());
    println!("cargo:rustc-env=KUKU_BUILD_REVISION={revision}");
    let docs = if Path::new("docs").is_dir() {
        Path::new("docs")
    } else {
        Path::new("../../docs")
    };
    let docs = fs::canonicalize(docs).expect("locate the canonical docs directory");
    println!("cargo:rerun-if-changed={}", docs.display());
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"));
    let skill = Path::new("skills/kuku-docs/SKILL.md");
    println!("cargo:rerun-if-changed={}", skill.display());
    let fingerprint =
        docs::build(&docs, &revision, &out, skill).expect("build the embedded docs bundle");
    println!("cargo:rustc-env=KUKU_DOCS_FINGERPRINT={fingerprint}");

    let version = env!("CARGO_PKG_VERSION");
    if git_revision.is_none() {
        println!("cargo:rustc-env=KUKU_VERSION={version}");
        return;
    }
    watch_git_path("HEAD");
    watch_git_path("refs/tags");
    watch_git_path("packed-refs");
    if let Some(reference) = git(&["symbolic-ref", "--quiet", "HEAD"]) {
        watch_git_path(&reference);
    }
    let tag = git(&[
        "describe",
        "--tags",
        "--exact-match",
        "--match",
        "v[0-9]*",
        "HEAD",
    ]);
    let version = tag
        .as_deref()
        .and_then(|tag| tag.strip_prefix('v'))
        .unwrap_or(version);
    let revision = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    println!("cargo:rustc-env=KUKU_VERSION={version} ({revision})");
}

fn source_revision(git_revision: Option<&str>) -> String {
    println!("cargo:rerun-if-changed=.source-revision");
    println!("cargo:rerun-if-env-changed=KUKU_SOURCE_REVISION");
    let vcs_info = Path::new(".cargo_vcs_info.json");
    let revision = if vcs_info.is_file() {
        println!("cargo:rerun-if-changed={}", vcs_info.display());
        fs::read(vcs_info)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|value| value["git"]["sha1"].as_str().map(str::to_owned))
    } else if let Some(revision) = git_revision {
        Some(revision.to_owned())
    } else {
        fs::read_to_string(".source-revision")
            .ok()
            .map(|revision| revision.trim().to_owned())
    };
    revision
        .filter(|revision| is_revision(revision))
        .or_else(|| {
            std::env::var("KUKU_SOURCE_REVISION")
                .ok()
                .filter(|revision| is_revision(revision))
        })
        .map(|revision| revision.to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_revision(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn watch_git_path(name: &str) {
    if let Some(path) = git(&["rev-parse", "--git-path", name])
        && Path::new(&path).exists()
    {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
