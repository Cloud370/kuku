use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{Acknowledgement, Client, Command, Error, Target};

/// Filesystem-backed pending commands that survive closing the client.
#[derive(Debug, Clone)]
pub struct PendingStore {
    path: PathBuf,
}

impl PendingStore {
    /// Open or create a private directory for pending command files.
    pub fn open(path: &Path) -> Result<Self, Error> {
        kuku::util::private_directory(path)?;
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    /// Persist the command before starting its network request.
    pub fn save(&self, command: &Command) -> Result<(), Error> {
        let _lock = self.lock()?;
        let path = self.command_path(&command.target, &command.command_id)?;
        if let Some(existing) = read(&path)? {
            return check_same(&existing, command);
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.path)?;
        serde_json::to_writer(&mut file, command).map_err(io::Error::other)?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist_noclobber(&path).map_err(|error| error.error)?;
        self.sync_directory()?;
        Ok(())
    }

    /// List the recoverable commands still held in this store.
    pub fn list(&self) -> Result<Vec<Command>, Error> {
        let _lock = self.lock()?;
        let mut commands = Vec::new();
        for entry in fs::read_dir(&self.path)? {
            let entry = entry?;
            if entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                if let Some(command) = read(&entry.path())? {
                    commands.push(command);
                }
            }
        }
        commands.sort_by(|left, right| {
            (
                &left.target.workspace,
                &left.target.session_id,
                &left.command_id,
            )
                .cmp(&(
                    &right.target.workspace,
                    &right.target.session_id,
                    &right.command_id,
                ))
        });
        Ok(commands)
    }

    /// Remove a confirmed or deliberately discarded command by its exact intent.
    pub fn remove(&self, command: &Command) -> Result<(), Error> {
        let _lock = self.lock()?;
        let path = self.command_path(&command.target, &command.command_id)?;
        if let Some(existing) = read(&path)? {
            check_same(&existing, command)?;
            fs::remove_file(path)?;
            self.sync_directory()?;
        }
        Ok(())
    }

    /// Persist and issue a command, removing it when the exchange has a definitive outcome.
    pub async fn submit(
        &self,
        client: &Client,
        command: &Command,
    ) -> Result<Acknowledgement, Error> {
        self.save(command)?;
        self.submit_saved(client, command).await
    }

    /// Submit an already saved intent and clear it when the exchange has a definitive outcome.
    pub async fn submit_saved(
        &self,
        client: &Client,
        command: &Command,
    ) -> Result<Acknowledgement, Error> {
        let result = client.submit(command).await;
        match &result {
            Ok(_) => self.remove(command)?,
            Err(error) if error.definitively_rejected() => self.remove(command)?,
            Err(_) => {}
        }
        result
    }

    /// Query and verify the receipt on reopening, keeping absent commands available for explicit retry.
    pub async fn reconcile(
        &self,
        client: &Client,
        command: &Command,
    ) -> Result<Option<Acknowledgement>, Error> {
        let result = client.recover(command).await?;
        if result.is_some() {
            self.remove(command)?;
        }
        Ok(result)
    }

    pub(crate) fn for_service(data_dir: &Path, scope: &str) -> Result<Self, Error> {
        fs::create_dir_all(data_dir)?;
        let root = data_dir.join("client-pending");
        kuku::util::private_directory(&root)?;
        Self::open(&root.join(format!("{:x}", Sha256::digest(scope.as_bytes()))))
    }

    fn command_path(&self, target: &Target, command_id: &str) -> Result<PathBuf, Error> {
        let identity = serde_json::to_vec(&(&target.workspace, &target.session_id, command_id))
            .map_err(io::Error::other)?;
        Ok(self
            .path
            .join(format!("{:x}.json", Sha256::digest(identity))))
    }

    fn lock(&self) -> io::Result<File> {
        let lock = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.path.join("lock"))?;
        lock.lock()?;
        Ok(lock)
    }

    fn sync_directory(&self) -> io::Result<()> {
        #[cfg(unix)]
        File::open(&self.path)?.sync_all()?;
        Ok(())
    }
}

fn read(path: &Path) -> Result<Option<Command>, Error> {
    match File::open(path) {
        Ok(file) => serde_json::from_reader(file).map(Some).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("read pending command {}: {error}", path.display()),
            )
            .into()
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn check_same(existing: &Command, command: &Command) -> Result<(), Error> {
    if existing == command {
        Ok(())
    } else {
        Err(Error::PendingConflict(command.command_id.clone()))
    }
}
