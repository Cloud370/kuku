use std::collections::BTreeMap;
use std::path::Path;

use crate::error::Result;

use super::hook::{HookEvent, HookInstance};
use super::loader::{LoadedPackage, Tier};

#[derive(Debug, Clone)]
pub struct PluginRegistry {
    packages: BTreeMap<String, LoadedPackage>,
    hooks: BTreeMap<HookEvent, Vec<HookInstance>>,
    skill_dirs: Vec<(std::path::PathBuf, Tier)>,
    hash: String,
    names: Vec<String>,
}

impl PartialEq for PluginRegistry {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
    }
}

impl Eq for PluginRegistry {}

impl PluginRegistry {
    pub fn builder() -> PluginRegistryBuilder {
        PluginRegistryBuilder::default()
    }

    pub fn hooks_for(&self, event: HookEvent) -> &[HookInstance] {
        self.hooks.get(&event).map_or(&[], |v| v.as_slice())
    }

    pub fn packages(&self) -> &BTreeMap<String, LoadedPackage> {
        &self.packages
    }

    pub fn skill_dirs(&self) -> &[(std::path::PathBuf, Tier)] {
        &self.skill_dirs
    }

    pub fn hash(&self) -> &str {
        &self.hash
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn len(&self) -> usize {
        self.packages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }
}

#[derive(Default)]
pub struct PluginRegistryBuilder {
    packages: Vec<LoadedPackage>,
}

impl PluginRegistryBuilder {
    pub fn load_packages(mut self, kuku_home: &Path, workspace: &Path) -> Result<Self> {
        let packages = super::loader::discover_packages(kuku_home, workspace)?;
        self.packages = packages;
        Ok(self)
    }

    pub fn build(self) -> crate::error::Result<PluginRegistry> {
        let mut packages = BTreeMap::new();
        let mut hooks: BTreeMap<HookEvent, Vec<HookInstance>> = BTreeMap::new();

        for pkg in &self.packages {
            let instances = super::hook::build_hook_instances(&pkg.manifest, &pkg.root, &pkg.name)?;

            for inst in &instances {
                hooks.entry(inst.event).or_default().push(inst.clone());
            }
            packages.insert(pkg.name.clone(), pkg.clone());
        }

        let skill_dirs = super::loader::collect_skill_dirs(&self.packages);
        let hash = compute_registry_hash(&packages);
        let names: Vec<String> = packages.keys().cloned().collect();

        Ok(PluginRegistry {
            packages,
            hooks,
            skill_dirs,
            hash,
            names,
        })
    }
}

fn compute_registry_hash(packages: &BTreeMap<String, LoadedPackage>) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for pkg in packages.values() {
        let manifest_hash = super::manifest::compute_manifest_hash(&pkg.manifest);
        hasher.update(manifest_hash.as_bytes());
        hasher.update(b"\n");
    }
    format!("sha256:{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_registry() {
        let reg = PluginRegistry::builder().build().unwrap();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.hooks_for(HookEvent::SessionStart).is_empty());
    }

    #[test]
    fn hash_is_deterministic() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg_dir = tmp.path().join("packages").join("test-pkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("kuku.toml"),
            "[package]\nname = \"test-pkg\"\nversion = \"1.0.0\"\n\n[[hooks]]\nevent = \"session.start\"\ncommand = \"hooks/a.sh\"\n",
        )
        .unwrap();

        let home = tmp.path().join("home");
        let r1 = PluginRegistry::builder()
            .load_packages(&home, tmp.path())
            .unwrap()
            .build()
            .unwrap();
        let r2 = PluginRegistry::builder()
            .load_packages(&home, tmp.path())
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(r1.hash(), r2.hash());
    }

    #[test]
    fn hooks_grouped_by_event() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg_dir = tmp.path().join(".kuku").join("packages").join("test-pkg");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("kuku.toml"),
            "[package]\nname = \"test-pkg\"\nversion = \"1.0.0\"\n\n[[hooks]]\nevent = \"session.start\"\ncommand = \"hooks/a.sh\"\n\n[[hooks]]\nevent = \"tool.pre_execute\"\ncommand = \"hooks/b.sh\"\n",
        )
        .unwrap();

        let home = tmp.path().join("home");
        let reg = PluginRegistry::builder()
            .load_packages(&home, tmp.path())
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(reg.hooks_for(HookEvent::SessionStart).len(), 1);
        assert_eq!(reg.hooks_for(HookEvent::ToolPreExecute).len(), 1);
        assert!(reg.hooks_for(HookEvent::SessionEnd).is_empty());
    }
}
