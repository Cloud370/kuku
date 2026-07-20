use std::future::Future;
use std::io::{Read, Write};
use std::pin::Pin;
use std::sync::Arc;

use cap_std::fs::{Dir, File, OpenOptions, OpenOptionsExt};

use super::{
    invalid_request, open_directory_components, open_directory_no_follow, unavailable,
    NormalizedRelativePath, ProcessCancellation, ProcessChunk, ProcessChunkSink, ProcessLimits,
    ProcessStream, RootCommand, WorkspaceCapability,
};

struct CommandOutputSink {
    events: Option<tokio::sync::mpsc::Sender<kuku::WorkspaceCommandEvent>>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl ProcessChunkSink for CommandOutputSink {
    fn push<'a>(
        &'a mut self,
        chunk: ProcessChunk,
    ) -> Pin<Box<dyn Future<Output = Result<(), crate::api::ApiError>> + Send + 'a>> {
        Box::pin(async move {
            let bytes = chunk.bytes().to_vec();
            match chunk.stream() {
                ProcessStream::Stdout => {
                    self.stdout.extend_from_slice(&bytes);
                    if let Some(events) = &self.events {
                        events
                            .send(kuku::WorkspaceCommandEvent::Stdout(bytes))
                            .await
                            .map_err(|_| unavailable("workspace command output consumer closed"))?;
                    }
                }
                ProcessStream::Stderr => {
                    self.stderr.extend_from_slice(&bytes);
                    if let Some(events) = &self.events {
                        events
                            .send(kuku::WorkspaceCommandEvent::Stderr(bytes))
                            .await
                            .map_err(|_| unavailable("workspace command output consumer closed"))?;
                    }
                }
            }
            Ok(())
        })
    }
}

impl WorkspaceCapability {
    pub fn query(
        &self,
        prompt: impl Into<String>,
        execution_scope: kuku::ExecutionScope,
        event_store: kuku::event::EventStore,
        selected_skills: Vec<kuku::event::SkillContextFact>,
    ) -> Result<kuku::Query, crate::api::ApiError> {
        if execution_scope.workspace_id != self.workspace_id {
            return Err(invalid_request(
                "execution scope does not belong to this workspace capability",
            ));
        }
        self.process_root.verify_execution_path()?;
        Ok(kuku::query(prompt).task_context(
            kuku::TaskQueryContext::new(execution_scope, event_store, Arc::new(self.clone()))
                .with_selected_skills(selected_skills),
        ))
    }
}

impl kuku::WorkspaceQueryCapability for WorkspaceCapability {
    fn workspace_id(&self) -> &str {
        self.workspace_id.as_str()
    }

    fn verify_identity(&self) -> kuku::Result<()> {
        self.process_root.verify_execution_path().map_err(|_| {
            kuku::Error::WorkspaceUnavailable("workspace identity changed".to_string())
        })
    }

    fn file_exists(&self, relative_path: &str) -> kuku::Result<bool> {
        let relative = NormalizedRelativePath::parse(relative_path)
            .map_err(|_| invalid_workspace_access("workspace file path is invalid"))?;
        let components: Vec<_> = relative.as_path().components().collect();
        let (last, parents) = components
            .split_last()
            .ok_or_else(|| invalid_workspace_access("workspace file path is invalid"))?;
        let std::path::Component::Normal(segment) = last else {
            return Err(invalid_workspace_access("workspace file path is invalid"));
        };
        let parent = open_directory_components(&self.root, parents)
            .map_err(|_| invalid_workspace_access("workspace parent is unavailable"))?;
        match parent.symlink_metadata(segment) {
            Ok(metadata) => Ok(!metadata.file_type().is_symlink() && metadata.is_file()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(invalid_workspace_access("workspace file is unavailable")),
        }
    }

    fn read_file(&self, relative_path: &str, max_bytes: usize) -> kuku::Result<Vec<u8>> {
        let relative = NormalizedRelativePath::parse(relative_path)
            .map_err(|_| invalid_workspace_access("workspace file path is invalid"))?;
        let file = self
            .open_file(&relative)
            .map_err(|_| invalid_workspace_access("workspace file is unavailable"))?;
        let mut bytes = Vec::new();
        file.take(max_bytes.saturating_add(1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > max_bytes {
            return Err(invalid_workspace_access(
                "workspace file exceeds the read limit",
            ));
        }
        Ok(bytes)
    }

    fn read_skill_source(
        &self,
        selected: &kuku::event::SkillContextFact,
        max_bytes: usize,
    ) -> kuku::Result<Vec<u8>> {
        if !matches!(
            selected.source.scope,
            kuku::event::SourceScope::Project | kuku::event::SourceScope::Workspace
        ) {
            return Err(kuku::Error::InvalidTaskContext(format!(
                "selected skill source scope is unsupported: {}",
                selected.skill_id
            )));
        }
        let relative_path = selected.source.relative_path.as_ref().ok_or_else(|| {
            kuku::Error::InvalidTaskContext(format!(
                "selected skill has no workspace-relative source: {}",
                selected.skill_id
            ))
        })?;
        self.read_file(relative_path.as_str(), max_bytes)
    }

    fn write_file(
        &self,
        relative_path: &str,
        contents: &[u8],
        max_bytes: usize,
    ) -> kuku::Result<()> {
        if contents.len() > max_bytes {
            return Err(invalid_workspace_access(
                "workspace write exceeds the write limit",
            ));
        }
        let relative = NormalizedRelativePath::parse(relative_path)
            .map_err(|_| invalid_workspace_access("workspace file path is invalid"))?;
        write_capability_file(&self.root, &relative, contents)
            .map_err(|_| invalid_workspace_access("workspace file cannot be written"))
    }

    fn list_entries(
        &self,
        relative_path: &str,
        max_entries: usize,
    ) -> kuku::Result<Vec<kuku::WorkspaceEntry>> {
        if max_entries == 0 {
            return Err(invalid_workspace_access(
                "workspace entry limit must be positive",
            ));
        }
        let (directory, prefix) = if matches!(relative_path, "" | ".") {
            (
                self.root
                    .try_clone()
                    .map_err(|_| invalid_workspace_access("workspace root is unavailable"))?,
                String::new(),
            )
        } else {
            let relative = NormalizedRelativePath::parse(relative_path)
                .map_err(|_| invalid_workspace_access("workspace directory path is invalid"))?;
            (
                self.open_dir(&relative)
                    .map_err(|_| invalid_workspace_access("workspace directory is unavailable"))?,
                relative_path.to_string(),
            )
        };
        let mut entries = Vec::new();
        collect_capability_entries(&directory, &prefix, 0, max_entries, &mut entries)
            .map_err(|_| invalid_workspace_access("workspace entries are unavailable"))?;
        Ok(entries)
    }

    fn run_command<'a>(
        &'a self,
        request: kuku::WorkspaceCommandRequest,
        events: Option<tokio::sync::mpsc::Sender<kuku::WorkspaceCommandEvent>>,
        cancellation: kuku::WorkspaceCommandCancellation,
    ) -> Pin<Box<dyn Future<Output = kuku::Result<kuku::WorkspaceCommandOutput>> + Send + 'a>> {
        Box::pin(async move {
            #[cfg(windows)]
            let command = RootCommand::new("cmd").args(["/C", request.command.as_str()]);
            #[cfg(not(windows))]
            let command = RootCommand::new("sh").args(["-c", request.command.as_str()]);
            let limits = ProcessLimits::new(request.timeout, request.max_output_bytes)
                .map_err(|_| invalid_workspace_access("workspace process limits are invalid"))?;
            let started = std::time::Instant::now();
            let process_cancellation = ProcessCancellation::new();
            let cancellation_bridge = process_cancellation.clone();
            let bridge = tokio::spawn(async move {
                cancellation.cancelled().await;
                cancellation_bridge.cancel();
            });
            let mut sink = CommandOutputSink {
                events,
                stdout: Vec::new(),
                stderr: Vec::new(),
            };
            let status = self
                .stream_at_root_with_cancellation(
                    command,
                    limits,
                    &mut sink,
                    Some(process_cancellation),
                )
                .await;
            bridge.abort();
            let status = status
                .map_err(|_| invalid_workspace_access("workspace process cannot be executed"))?;
            Ok(kuku::WorkspaceCommandOutput {
                exit_code: status.code(),
                timed_out: status.timed_out(),
                cancelled: status.cancelled(),
                stdout: sink.stdout,
                stderr: sink.stderr,
                duration_ms: started.elapsed().as_millis() as u64,
            })
        })
    }
}

fn invalid_workspace_access(message: &str) -> kuku::Error {
    kuku::Error::WorkspaceUnavailable(message.to_string())
}

fn write_capability_file(
    root: &Dir,
    relative: &NormalizedRelativePath,
    contents: &[u8],
) -> Result<(), crate::api::ApiError> {
    let components: Vec<_> = relative.as_path().components().collect();
    let (last, parents) = components
        .split_last()
        .ok_or_else(|| invalid_request("workspace path must not be empty"))?;
    let std::path::Component::Normal(segment) = last else {
        return Err(invalid_request("workspace path is not normalized"));
    };
    let parent = open_directory_components(root, parents)?;
    match parent.symlink_metadata(segment) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(invalid_request(
                "workspace write target must be a regular file",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(unavailable("workspace write target is unavailable")),
    }
    let temporary = format!(
        ".kuku-write-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::GENERIC_WRITE;
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE,
        };

        options
            .access_mode(GENERIC_WRITE | DELETE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let result = (|| {
        let mut file = parent
            .open_with(&temporary, &options)
            .map_err(|_| unavailable("workspace temporary file cannot be created"))?;
        file.write_all(contents)
            .map_err(|_| unavailable("workspace file cannot be written"))?;
        file.sync_data()
            .map_err(|_| unavailable("workspace file cannot be synchronized"))?;
        replace_capability_file(&file, &parent, &temporary, segment)
            .map_err(|_| unavailable("workspace file cannot be replaced"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = parent.remove_file(&temporary);
    }
    result
}

#[cfg(not(windows))]
fn replace_capability_file(
    _file: &File,
    parent: &Dir,
    temporary: &str,
    destination: &std::ffi::OsStr,
) -> std::io::Result<()> {
    parent.rename(temporary, parent, destination)
}

#[cfg(windows)]
fn replace_capability_file(
    file: &File,
    parent: &Dir,
    _temporary: &str,
    destination: &std::ffi::OsStr,
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileRenameInfo, SetFileInformationByHandle, FILE_RENAME_INFO,
    };

    let destination: Vec<u16> = destination.encode_wide().collect();
    let header = std::mem::size_of::<FILE_RENAME_INFO>() - std::mem::size_of::<u16>();
    let mut buffer = vec![0_u8; header + destination.len() * std::mem::size_of::<u16>()];
    let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = true;
        (*info).RootDirectory = parent.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
        (*info).FileNameLength = (destination.len() * std::mem::size_of::<u16>()) as u32;
        std::ptr::copy_nonoverlapping(
            destination.as_ptr(),
            std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
            destination.len(),
        );
        if SetFileInformationByHandle(
            file.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE,
            FileRenameInfo,
            buffer.as_ptr().cast(),
            buffer.len() as u32,
        ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

fn collect_capability_entries(
    directory: &Dir,
    prefix: &str,
    depth: usize,
    max_entries: usize,
    entries: &mut Vec<kuku::WorkspaceEntry>,
) -> Result<(), crate::api::ApiError> {
    const MAX_DEPTH: usize = 64;
    let read_dir = directory
        .read_dir(".")
        .map_err(|_| unavailable("workspace directory entries are unavailable"))?;
    let remaining = max_entries.saturating_sub(entries.len());
    let mut children = Vec::with_capacity(remaining.min(1024));
    for entry in read_dir {
        let entry = entry.map_err(|_| unavailable("workspace entry is unavailable"))?;
        let name = entry.file_name();
        if name.to_str().is_some_and(is_default_excluded_directory)
            && entry
                .file_type()
                .map_err(|_| unavailable("workspace entry metadata is unavailable"))?
                .is_dir()
        {
            continue;
        }
        if children.len() >= remaining {
            return Err(unavailable("workspace entry limit was exceeded"));
        }
        children.push(entry);
    }
    children.sort_by_key(|entry| entry.file_name());
    for entry in children {
        if entries.len() >= max_entries {
            return Err(unavailable("workspace entry limit was exceeded"));
        }
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| unavailable("workspace entry name is not UTF-8"))?;
        let metadata = directory
            .symlink_metadata(name)
            .map_err(|_| unavailable("workspace entry metadata is unavailable"))?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        let path = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}/{name}")
        };
        if path.len() > 4096 {
            return Err(unavailable("workspace entry path limit was exceeded"));
        }
        let is_dir = metadata.is_dir();
        let is_file = metadata.is_file();
        entries.push(kuku::WorkspaceEntry {
            path: path.clone(),
            is_file,
            is_dir,
        });
        if is_dir && depth >= MAX_DEPTH {
            return Err(unavailable("workspace traversal depth was exceeded"));
        }
        if is_dir {
            let opened = open_directory_no_follow(directory, std::ffi::OsStr::new(name))
                .map_err(|_| unavailable("workspace directory is unavailable"))?;
            collect_capability_entries(&opened, &path, depth + 1, max_entries, entries)?;
        }
    }
    Ok(())
}

fn is_default_excluded_directory(name: &str) -> bool {
    matches!(name, ".git" | "target" | "node_modules")
}
