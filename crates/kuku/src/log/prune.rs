use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::config::LogsConfig;
use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneCandidate {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub modified_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrunePlan {
    pub remove: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PruneOptions {
    active_paths: Vec<PathBuf>,
}

impl PruneOptions {
    pub fn with_active_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.active_paths.push(path.into());
        self
    }

    pub fn excludes_active_path(&self, path: &Path) -> bool {
        self.active_paths
            .iter()
            .any(|active_path| active_path == path)
    }
}

#[derive(Debug, Clone)]
pub struct StartupPruneGate {
    interval: Duration,
    last_pruned_at_by_home: std::collections::HashMap<PathBuf, SystemTime>,
}

impl StartupPruneGate {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_pruned_at_by_home: std::collections::HashMap::new(),
        }
    }

    pub fn should_prune(&mut self, kuku_home: &Path, now: SystemTime) -> bool {
        let kuku_home = normalized_home(kuku_home);
        let should_prune = self
            .last_pruned_at_by_home
            .get(&kuku_home)
            .copied()
            .and_then(|last| now.duration_since(last).ok())
            .is_none_or(|elapsed| elapsed >= self.interval);
        if should_prune {
            self.last_pruned_at_by_home.insert(kuku_home, now);
        }
        should_prune
    }
}

fn normalized_home(kuku_home: &Path) -> PathBuf {
    std::fs::canonicalize(kuku_home).unwrap_or_else(|_| kuku_home.to_path_buf())
}

pub fn select_prunable_files(
    limits: &LogsConfig,
    now: SystemTime,
    candidates: Vec<PruneCandidate>,
) -> PrunePlan {
    select_prunable_files_with_options(limits, now, candidates, &PruneOptions::default())
}

pub fn select_prunable_files_with_options(
    limits: &LogsConfig,
    now: SystemTime,
    mut candidates: Vec<PruneCandidate>,
    options: &PruneOptions,
) -> PrunePlan {
    candidates.retain(|candidate| {
        candidate.path.file_name().and_then(|name| name.to_str()) != Some("events.jsonl")
            && !options
                .active_paths
                .iter()
                .any(|active_path| active_path == &candidate.path)
    });
    candidates.sort_by_key(|candidate| candidate.modified_at);

    let age_cutoff = now
        .checked_sub(Duration::from_secs(
            u64::from(limits.max_age_days) * 24 * 60 * 60,
        ))
        .unwrap_or(SystemTime::UNIX_EPOCH);

    let mut remove = Vec::new();
    let mut retained = Vec::new();

    for candidate in candidates {
        if candidate.modified_at < age_cutoff {
            remove.push(candidate.path);
        } else {
            retained.push(candidate);
        }
    }

    let max_total_size = u64::from(limits.max_total_size_mb) * 1024 * 1024;
    let mut total_size: u64 = retained.iter().map(|candidate| candidate.size_bytes).sum();
    let mut retained_iter = retained.into_iter();

    while total_size > max_total_size {
        let Some(candidate) = retained_iter.next() else {
            break;
        };
        total_size = total_size.saturating_sub(candidate.size_bytes);
        remove.push(candidate.path);
    }

    PrunePlan { remove }
}

pub fn prune_logs(
    kuku_home: &Path,
    limits: &LogsConfig,
    now: SystemTime,
    options: PruneOptions,
) -> Result<PrunePlan> {
    let root = crate::log::logs_root(kuku_home);
    let candidates = collect_candidates(&root)?;
    let plan = select_prunable_files_with_options(limits, now, candidates, &options);
    for path in &plan.remove {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
    }
    Ok(plan)
}

fn collect_candidates(root: &Path) -> Result<Vec<PruneCandidate>> {
    let mut candidates = Vec::new();
    if !root.exists() {
        return Ok(candidates);
    }
    collect_candidates_inner(root, &mut candidates)?;
    Ok(candidates)
}

fn collect_candidates_inner(root: &Path, candidates: &mut Vec<PruneCandidate>) -> Result<()> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_candidates_inner(&path, candidates)?;
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let metadata = entry.metadata()?;
        candidates.push(PruneCandidate {
            path,
            size_bytes: metadata.len(),
            modified_at: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_selection_removes_oldest_files_after_age_filter() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(40 * 24 * 60 * 60);
        let limits = LogsConfig {
            max_age_days: 14,
            max_total_size_mb: 1,
        };
        let plan = select_prunable_files(
            &limits,
            now,
            vec![
                PruneCandidate {
                    path: PathBuf::from("old.jsonl"),
                    size_bytes: 100,
                    modified_at: SystemTime::UNIX_EPOCH,
                },
                PruneCandidate {
                    path: PathBuf::from("newer.jsonl"),
                    size_bytes: 2 * 1024 * 1024,
                    modified_at: now,
                },
            ],
        );

        assert_eq!(
            plan.remove,
            vec![PathBuf::from("old.jsonl"), PathBuf::from("newer.jsonl")]
        );
    }

    #[test]
    fn prune_selection_applies_age_before_size_budget() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(40 * 24 * 60 * 60);
        let limits = LogsConfig {
            max_age_days: 14,
            max_total_size_mb: 1,
        };

        let plan = select_prunable_files(
            &limits,
            now,
            vec![
                PruneCandidate {
                    path: PathBuf::from("new-large.jsonl"),
                    size_bytes: 2 * 1024 * 1024,
                    modified_at: now,
                },
                PruneCandidate {
                    path: PathBuf::from("old-small.jsonl"),
                    size_bytes: 1,
                    modified_at: SystemTime::UNIX_EPOCH,
                },
            ],
        );

        assert_eq!(
            plan.remove,
            vec![
                PathBuf::from("old-small.jsonl"),
                PathBuf::from("new-large.jsonl")
            ]
        );
    }

    #[test]
    fn prune_selection_excludes_active_paths_from_age_and_size_removal() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(40 * 24 * 60 * 60);
        let active = PathBuf::from("active.jsonl");
        let limits = LogsConfig {
            max_age_days: 14,
            max_total_size_mb: 1,
        };

        let plan = select_prunable_files_with_options(
            &limits,
            now,
            vec![
                PruneCandidate {
                    path: active.clone(),
                    size_bytes: 3 * 1024 * 1024,
                    modified_at: SystemTime::UNIX_EPOCH,
                },
                PruneCandidate {
                    path: PathBuf::from("old.jsonl"),
                    size_bytes: 1,
                    modified_at: SystemTime::UNIX_EPOCH,
                },
            ],
            &PruneOptions::default().with_active_path(active),
        );

        assert_eq!(plan.remove, vec![PathBuf::from("old.jsonl")]);
    }

    #[test]
    fn prune_logs_derives_logs_root_and_never_removes_events_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let logs = crate::log::logs_root(home);
        let runtime = logs.join("runtime");
        let session = home.join("p/code/workspace/sessions/s_1");
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::create_dir_all(&session).unwrap();
        let old_log = runtime.join("old.jsonl");
        std::fs::write(&old_log, vec![b'x'; 2 * 1024 * 1024]).unwrap();
        std::fs::write(runtime.join("events.jsonl"), vec![b'e'; 2 * 1024 * 1024]).unwrap();
        std::fs::write(runtime.join("keep.txt"), "not a log\n").unwrap();
        std::fs::write(session.join("events.jsonl"), "event\n").unwrap();

        let limits = LogsConfig {
            max_age_days: 14,
            max_total_size_mb: 1,
        };
        let plan = prune_logs(home, &limits, SystemTime::now(), PruneOptions::default()).unwrap();

        assert_eq!(plan.remove, vec![old_log.clone()]);
        assert!(!old_log.exists());
        assert!(runtime.join("events.jsonl").exists());
        assert!(runtime.join("keep.txt").exists());
        assert!(runtime.exists());
        assert_eq!(
            std::fs::read_to_string(session.join("events.jsonl")).unwrap(),
            "event\n"
        );
    }

    #[test]
    fn startup_prune_gate_is_low_frequency() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(60 * 60);
        let mut gate = StartupPruneGate::new(Duration::from_secs(24 * 60 * 60));

        assert!(gate.should_prune(Path::new("/tmp/kuku-a"), now));
        assert!(!gate.should_prune(Path::new("/tmp/kuku-a"), now + Duration::from_secs(60)));
        assert!(gate.should_prune(
            Path::new("/tmp/kuku-a"),
            now + Duration::from_secs(25 * 60 * 60)
        ));
    }

    #[test]
    fn startup_prune_gate_tracks_each_home_independently() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(60 * 60);
        let mut gate = StartupPruneGate::new(Duration::from_secs(24 * 60 * 60));

        assert!(gate.should_prune(Path::new("/tmp/kuku-a"), now));
        assert!(gate.should_prune(Path::new("/tmp/kuku-b"), now + Duration::from_secs(60)));
        assert!(!gate.should_prune(Path::new("/tmp/kuku-a"), now + Duration::from_secs(120)));
    }
}
