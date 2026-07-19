use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

fn private_file(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn temp_path(path: &Path) -> io::Result<PathBuf> {
    let mut random = [0u8; 12];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    let suffix = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(path.with_file_name(format!(
        ".{}.tmp-{suffix}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("state")
    )))
}

pub fn write_private_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = temp_path(path)?;
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).truncate(true);
        let mut file = options.open(&temporary)?;
        private_file(&file)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
