use std::path::{Path, PathBuf};

use super::{ensure_path_within_workspace, RevertPlan};

#[derive(Debug)]
pub(crate) struct FileRevertTransaction {
    backups: Vec<(PathBuf, Option<Vec<u8>>)>,
}

impl FileRevertTransaction {
    pub(crate) fn rollback(self) -> Result<(), std::io::Error> {
        for (path, original) in self.backups.into_iter().rev() {
            match original {
                Some(bytes) => crate::tool::builtin::common::write_atomically(&path, &bytes)?,
                None if path.exists() => std::fs::remove_file(path)?,
                None => {}
            }
        }
        Ok(())
    }
}

pub(crate) fn apply_message_rewind_files(
    plan: &RevertPlan,
    workspace: &Path,
    session_dir: &Path,
    rewind_event_id: u64,
) -> Result<(FileRevertTransaction, Vec<String>), std::io::Error> {
    apply_message_rewind_files_inner(plan, workspace, session_dir, rewind_event_id, None)
}

pub(super) fn apply_message_rewind_files_inner(
    plan: &RevertPlan,
    workspace: &Path,
    session_dir: &Path,
    rewind_event_id: u64,
    fail_after: Option<usize>,
) -> Result<(FileRevertTransaction, Vec<String>), std::io::Error> {
    let canonical_workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let backup_dir = session_dir.join(format!("pre-revert-{rewind_event_id}"));
    let paths = plan
        .restores
        .iter()
        .map(|restore| &restore.path)
        .chain(plan.deletes.iter());
    let mut backups = Vec::new();
    for path in paths {
        ensure_path_within_workspace(&canonical_workspace, path)?;
        let original = path.exists().then(|| std::fs::read(path)).transpose()?;
        if let (Some(bytes), Ok(relative)) = (&original, path.strip_prefix(&canonical_workspace)) {
            let backup_path = backup_dir.join(relative);
            if let Some(parent) = backup_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(backup_path, bytes)?;
        }
        backups.push((path.clone(), original));
    }
    let mut warnings = Vec::new();
    for restore in &plan.restores {
        if let Some((_, Some(current))) = backups.iter().find(|(path, _)| path == &restore.path) {
            if crate::tool::builtin::common::content_hash(current)
                != crate::tool::builtin::common::content_hash(
                    restore.new_content_on_plan.as_bytes(),
                )
            {
                warnings.push(format!(
                    "{}: file changed since plan computed",
                    restore.path.display()
                ));
            }
        }
    }
    let transaction = FileRevertTransaction { backups };
    let apply_result = (|| {
        let mut applied = 0;
        for restore in &plan.restores {
            crate::tool::builtin::common::write_atomically(
                &restore.path,
                restore.old_content.as_bytes(),
            )?;
            applied += 1;
            if fail_after == Some(applied) {
                return Err(std::io::Error::other("injected file restore failure"));
            }
        }
        for path in &plan.deletes {
            if path.exists() {
                std::fs::remove_file(path)?;
            }
            applied += 1;
            if fail_after == Some(applied) {
                return Err(std::io::Error::other("injected file restore failure"));
            }
        }
        Ok(())
    })();
    if let Err(error) = apply_result {
        return match transaction.rollback() {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(std::io::Error::other(format!(
                "file restore failed ({error}) and workspace rollback failed ({rollback_error})"
            ))),
        };
    }
    Ok((transaction, warnings))
}
