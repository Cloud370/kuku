use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

use super::manifest::{parse_manifest, PackageManifest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    User,
    Project,
}

impl From<Tier> for crate::skill::definition::SkillSource {
    fn from(tier: Tier) -> Self {
        match tier {
            Tier::User => crate::skill::definition::SkillSource::User,
            Tier::Project => crate::skill::definition::SkillSource::Project,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoadedPackage {
    pub name: String,
    pub tier: Tier,
    pub root: PathBuf,
    pub manifest: PackageManifest,
}

pub fn discover_packages(kuku_home: &Path, workspace: &Path) -> Result<Vec<LoadedPackage>> {
    let mut packages = Vec::new();

    let user_dir = kuku_home.join("packages");
    scan_packages_dir(&user_dir, Tier::User, &mut packages)?;

    let project_dir = workspace.join(".kuku").join("packages");
    scan_packages_dir(&project_dir, Tier::Project, &mut packages)?;

    Ok(packages)
}

fn scan_packages_dir(
    dir: &Path,
    tier: Tier,
    packages: &mut Vec<LoadedPackage>,
) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let toml_path = path.join("kuku.toml");
        if !toml_path.is_file() {
            continue;
        }
        let content = std::fs::read_to_string(&toml_path)
            .map_err(|e| Error::PluginManifest(toml_path.clone(), format!("cannot read: {e}")))?;
        let manifest = parse_manifest(&content, &toml_path)?;
        let name = manifest.package.name.clone();

        if let Some(existing) = packages.iter_mut().find(|p| p.name == name) {
            if tier == Tier::Project {
                *existing = LoadedPackage {
                    name,
                    tier,
                    root: path,
                    manifest,
                };
            }
            continue;
        }

        packages.push(LoadedPackage {
            name,
            tier,
            root: path,
            manifest,
        });
    }

    Ok(())
}

pub fn collect_skill_dirs(packages: &[LoadedPackage]) -> Vec<(PathBuf, Tier)> {
    packages
        .iter()
        .filter_map(|pkg| {
            let skills_dir = pkg.root.join("skills");
            if skills_dir.is_dir() {
                Some((skills_dir, pkg.tier))
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_package(dir: &Path, name: &str, version: &str) {
        let pkg_dir = dir.join(name);
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("kuku.toml"),
            format!(
                r#"
[package]
name = "{name}"
version = "{version}"

[[hooks]]
event = "session.start"
command = "hooks/a.sh"
"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn discover_user_and_project_packages() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(home.join("packages")).unwrap();
        std::fs::create_dir_all(workspace.join(".kuku/packages")).unwrap();

        setup_package(&home.join("packages"), "user-pkg", "1.0.0");
        setup_package(
            &workspace.join(".kuku/packages"),
            "proj-pkg",
            "2.0.0",
        );

        let pkgs = discover_packages(&home, &workspace).unwrap();
        assert_eq!(pkgs.len(), 2);
        assert!(pkgs
            .iter()
            .any(|p| p.name == "user-pkg" && p.tier == Tier::User));
        assert!(pkgs
            .iter()
            .any(|p| p.name == "proj-pkg" && p.tier == Tier::Project));
    }

    #[test]
    fn project_overrides_user() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(home.join("packages")).unwrap();
        std::fs::create_dir_all(workspace.join(".kuku/packages")).unwrap();

        setup_package(&home.join("packages"), "shared", "1.0.0");
        setup_package(
            &workspace.join(".kuku/packages"),
            "shared",
            "2.0.0",
        );

        let pkgs = discover_packages(&home, &workspace).unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].tier, Tier::Project);
        assert_eq!(pkgs[0].manifest.package.version, "2.0.0");
    }

    #[test]
    fn nonexistent_dir_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let pkgs =
            discover_packages(&tmp.path().join("nope"), &tmp.path().join("nope")).unwrap();
        assert!(pkgs.is_empty());
    }

    #[test]
    fn collect_skill_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg_dir = tmp.path().join("my-pkg");
        std::fs::create_dir_all(pkg_dir.join("skills/tdd")).unwrap();
        std::fs::write(
            pkg_dir.join("kuku.toml"),
            "[package]\nname = \"my-pkg\"\nversion = \"1.0.0\"\n\n[[hooks]]\nevent = \"session.start\"\ncommand = \"hooks/a.sh\"\n",
        )
        .unwrap();

        let mut direct = Vec::new();
        scan_packages_dir(tmp.path(), Tier::Project, &mut direct).unwrap();
        let dirs = super::collect_skill_dirs(&direct);
        assert_eq!(dirs.len(), 1);
    }
}
