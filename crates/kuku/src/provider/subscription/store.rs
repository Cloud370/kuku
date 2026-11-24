use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::config::{SubscriptionAccount, SubscriptionChannel};
use crate::util::{fs_sync, open_private_file, private_directory, sync_private_file};
use crate::{Error, Result};

const MAX_RECORD_BYTES: u64 = 64 * 1024;
const LOCK_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct AccountRecord {
    pub(super) channel: SubscriptionChannel,
    pub(super) revision: String,
    pub(super) client_id: String,
    pub(super) subject: String,
    pub(super) masked_email: Option<String>,
    pub(super) email_identifier: Option<String>,
    pub(super) tokens: Option<Tokens>,
}

#[derive(PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Tokens {
    pub(super) access: Option<String>,
    pub(super) refresh: Option<String>,
    pub(super) scope: String,
    pub(super) expires_at: i64,
}

#[derive(Serialize, Deserialize)]
pub(super) struct HostRegistration {
    pub(super) id: String,
    pub(super) email_key: [u8; 32],
}

pub(super) struct AccountStore<'a> {
    account: &'a SubscriptionAccount,
}

impl<'a> AccountStore<'a> {
    pub(super) fn new(account: &'a SubscriptionAccount) -> Result<Self> {
        account.validate_path()?;
        Ok(Self { account })
    }

    pub(super) fn read(&self) -> Result<Option<AccountRecord>> {
        let path = self.account.credential_file();
        if !exists(path)? {
            return Ok(None);
        }
        self.check_directories()?;
        let bytes = read_private(path)?;
        let record: AccountRecord = serde_json::from_slice(&bytes)
            .map_err(|_| Error::ConfigLoad("invalid subscription account file".into()))?;
        if record.channel != self.account.channel()
            || record.client_id.is_empty()
            || record.subject.is_empty()
        {
            return Err(Error::ConfigLoad(
                "subscription account file does not match its connection".into(),
            ));
        }
        Ok(Some(record))
    }

    pub(super) fn write(&self, record: &AccountRecord) -> Result<()> {
        let path = self.account.credential_file();
        let bytes = serde_json::to_vec(record)
            .map_err(|_| Error::ConfigLoad("cannot encode the subscription account".into()))?;
        write_private(path, &bytes)
    }

    pub(super) async fn host_registration(&self) -> Result<HostRegistration> {
        self.check_directories()?;
        let directory = self.account.credential_file().ancestors().nth(2).unwrap();
        let path = directory.join("host.json");
        let _lock = lock(&directory.join("host.lock")).await?;
        if exists(&path)? {
            let bytes = read_private(&path)?;
            let host: HostRegistration = serde_json::from_slice(&bytes)
                .map_err(|_| Error::ConfigLoad("invalid subscription host record".into()))?;
            if !host
                .id
                .strip_prefix("urn:uuid:")
                .is_some_and(|id| Uuid::parse_str(id).is_ok())
            {
                return Err(Error::ConfigLoad("invalid subscription host ID".into()));
            }
            return Ok(host);
        }
        let mut email_key = [0; 32];
        getrandom::fill(&mut email_key)
            .map_err(|_| Error::InvalidArgument("OS randomness is unavailable".into()))?;
        let host = HostRegistration {
            id: format!("urn:uuid:{}", Uuid::new_v4()),
            email_key,
        };
        write_private(&path, &serde_json::to_vec(&host)?)?;
        Ok(host)
    }

    pub(super) async fn lock(&self) -> Result<File> {
        self.check_directories()?;
        lock(&self.account.credential_file().with_extension("lock")).await
    }

    fn check_directories(&self) -> Result<()> {
        let path = self.account.credential_file();
        fs_sync::create_dir_all(path.ancestors().nth(4).unwrap())?;
        for level in [3, 2, 1] {
            private_directory(path.ancestors().nth(level).unwrap())?;
        }
        Ok(())
    }
}

pub(super) fn now() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

async fn lock(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    let deadline = tokio::time::Instant::now() + LOCK_TIMEOUT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(TryLockError::WouldBlock) => {
                return Err(Error::InvalidArgument(
                    "another process is updating this subscription account".into(),
                ));
            }
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

fn exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn read_private(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    open_private_file(path)?
        .take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(Error::ConfigLoad(
            "subscription credential file is too large".into(),
        ));
    }
    Ok(bytes)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(Error::ConfigLoad(
            "subscription credential file is too large".into(),
        ));
    }
    let mut staged = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    #[cfg(windows)]
    crate::util::set_config_temp_security(None, staged.path(), staged.as_file())?;
    staged.write_all(bytes)?;
    staged.as_file().sync_all()?;
    staged.persist(path).map_err(|error| error.error)?;
    sync_private_file(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{AccountRecord, AccountStore};
    use crate::config::{SubscriptionAccount, SubscriptionChannel};

    #[tokio::test]
    async fn named_accounts_share_a_host_and_lock_the_same_record() {
        let root = tempfile::tempdir().unwrap();
        let work =
            SubscriptionAccount::new(root.path(), SubscriptionChannel::ChatGpt, "work").unwrap();
        let personal =
            SubscriptionAccount::new(root.path(), SubscriptionChannel::ChatGpt, "personal")
                .unwrap();
        let work_store = AccountStore::new(&work).unwrap();
        let personal_store = AccountStore::new(&personal).unwrap();
        let (left, right) = tokio::join!(
            work_store.host_registration(),
            personal_store.host_registration()
        );
        let left = left.unwrap();
        let right = right.unwrap();
        assert_eq!(left.id, right.id);
        assert_eq!(left.email_key, right.email_key);
        let work_lock = work_store.lock().await.unwrap();
        let mut waiting = Box::pin(work_store.lock());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), &mut waiting)
                .await
                .is_err()
        );
        let _personal_lock = personal_store.lock().await.unwrap();
        work_store
            .write(&AccountRecord {
                channel: SubscriptionChannel::ChatGpt,
                revision: "new-generation".into(),
                client_id: "oaiapp_work".into(),
                subject: "identity".into(),
                masked_email: Some("a***@e***".into()),
                email_identifier: Some("local-fingerprint".into()),
                tokens: None,
            })
            .unwrap();
        drop(work_lock);
        let _work_lock = waiting.await.unwrap();
        let record = work_store.read().unwrap().unwrap();
        assert_eq!("new-generation", record.revision);
        assert!(personal_store.read().unwrap().is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let metadata = std::fs::metadata(work.credential_file()).unwrap();
            assert_eq!(0o600, metadata.permissions().mode() & 0o777);
        }
    }
}
