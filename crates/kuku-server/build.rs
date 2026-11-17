use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let version = env!("CARGO_PKG_VERSION");
    let Some(revision) = git(&["rev-parse", "--short", "HEAD"]) else {
        println!("cargo:rustc-env=KUKU_VERSION={version}");
        return;
    };
    watch_git_path("HEAD");
    if let Some(reference) = git(&["symbolic-ref", "--quiet", "HEAD"]) {
        watch_git_path(&reference);
        watch_git_path("packed-refs");
    }
    println!("cargo:rustc-env=KUKU_VERSION={version} ({revision})");
}

fn watch_git_path(name: &str) {
    if let Some(path) = git(&["rev-parse", "--git-path", name]) {
        if Path::new(&path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
