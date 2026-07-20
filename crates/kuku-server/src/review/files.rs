//! Implements bounded Review file reads through workspace capabilities.

use std::cmp;
use std::io::Read;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::api::{
    ApiError, ApiErrorCode, ApiVersion, FileContent, FileEntry, FileKind, FilePage, FileSearchPage,
    PageCursor, RevisionToken, SearchMatch, TextRange, WorkspaceId,
};
use crate::platform::WorkspaceCapability;
use crate::review::{
    ReviewAdmission, ReviewLimits, ReviewPermit, RevisionBudget, WorkspaceCapabilityProvider,
};

const TREE_DOMAIN: &[u8] = b"kuku.review.tree.v1";
const SEARCH_DOMAIN: &[u8] = b"kuku.review.search.v1";
const FILE_DOMAIN: &[u8] = b"kuku.review.file.v1";
const CURSOR_DOMAIN: &[u8] = b"kuku.review.cursor.v1";
const READ_BUFFER_BYTES: usize = 64 * 1024;

/// Provides bounded tree, search, content, and revision reads.
pub struct WorkspaceReadService {
    workspaces: Arc<dyn WorkspaceCapabilityProvider>,
    limits: ReviewLimits,
    admission: Arc<ReviewAdmission>,
}

impl WorkspaceReadService {
    /// Creates a service with the canonical Review limits.
    pub fn new(workspaces: Arc<dyn WorkspaceCapabilityProvider>) -> Self {
        Self::with_limits(workspaces, ReviewLimits::default())
    }

    /// Creates a service with explicitly supplied limits.
    pub fn with_limits(
        workspaces: Arc<dyn WorkspaceCapabilityProvider>,
        limits: ReviewLimits,
    ) -> Self {
        let admission = Arc::new(ReviewAdmission::new(&limits));
        Self {
            workspaces,
            limits,
            admission,
        }
    }

    /// Lists a stable page of logical entries beneath a workspace prefix.
    pub async fn tree(
        &self,
        workspace_id: &WorkspaceId,
        prefix: &str,
        cursor: Option<&PageCursor>,
        limit: u16,
    ) -> Result<FilePage, ApiError> {
        validate_limit(limit)?;
        validate_prefix(prefix, &self.limits)?;
        let capability = self.capability(workspace_id)?;
        let permit = self.admission.try_acquire_scan(workspace_id)?;
        let limits = self.limits.clone();
        let workspace_id = workspace_id.clone();
        let prefix = prefix.to_owned();
        let cursor = cursor.cloned();
        run_blocking(move || {
            tree_sync(
                capability,
                permit,
                limits,
                workspace_id,
                prefix,
                cursor,
                limit,
            )
        })
        .await
    }

    /// Searches path text only and returns a stable bounded page of matches.
    pub async fn search(
        &self,
        workspace_id: &WorkspaceId,
        prefix: &str,
        query: &str,
        cursor: Option<&PageCursor>,
        limit: u16,
    ) -> Result<FileSearchPage, ApiError> {
        validate_limit(limit)?;
        validate_prefix(prefix, &self.limits)?;
        if query.is_empty() || query.len() > self.limits.search_query_bytes {
            return Err(invalid_request("review search query is invalid"));
        }
        let capability = self.capability(workspace_id)?;
        let permit = self.admission.try_acquire_scan(workspace_id)?;
        let limits = self.limits.clone();
        let workspace_id = workspace_id.clone();
        let prefix = prefix.to_owned();
        let query = query.to_owned();
        let cursor = cursor.cloned();
        run_blocking(move || {
            search_sync(
                capability,
                permit,
                limits,
                SearchInput {
                    workspace_id,
                    prefix,
                    query,
                    cursor,
                    limit,
                },
            )
        })
        .await
    }

    /// Reads one bounded LF-normalized text range after hashing the complete file.
    pub async fn content(
        &self,
        workspace_id: &WorkspaceId,
        path: &str,
        start_line: u32,
        end_line: u32,
    ) -> Result<FileContent, ApiError> {
        validate_path(path, &self.limits)?;
        if start_line == 0 || end_line < start_line {
            return Err(invalid_request("review file line range is invalid"));
        }
        let capability = self.capability(workspace_id)?;
        let permit = self.admission.try_acquire_scan(workspace_id)?;
        let limits = self.limits.clone();
        let workspace_id = workspace_id.clone();
        let path = path.to_owned();
        run_blocking(move || {
            content_sync(
                capability,
                permit,
                limits,
                workspace_id,
                path,
                start_line,
                end_line,
            )
        })
        .await
    }

    /// Hashes one complete file into its canonical content revision.
    pub async fn current_revision(
        &self,
        workspace_id: &WorkspaceId,
        path: &str,
    ) -> Result<RevisionToken, ApiError> {
        validate_path(path, &self.limits)?;
        let capability = self.capability(workspace_id)?;
        let permit = self.admission.try_acquire_scan(workspace_id)?;
        let limits = self.limits.clone();
        let path = path.to_owned();
        run_blocking(move || {
            let _permit = permit;
            let mut budget = RevisionBudget::new(&limits);
            capture_file_with_retry(&capability, &path, &mut budget, CapturePurpose::File, false)
                .map(|capture| capture.revision)
        })
        .await
    }

    fn capability(&self, workspace_id: &WorkspaceId) -> Result<WorkspaceCapability, ApiError> {
        self.workspaces
            .capability(workspace_id)
            .map_err(redact_capability_error)
    }
}

async fn run_blocking<T, F>(operation: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ApiError> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| internal_error("review file worker stopped unexpectedly"))?
}

fn tree_sync(
    capability: WorkspaceCapability,
    _permit: ReviewPermit,
    limits: ReviewLimits,
    workspace_id: WorkspaceId,
    prefix: String,
    cursor: Option<PageCursor>,
    limit: u16,
) -> Result<FilePage, ApiError> {
    let mut budget = RevisionBudget::new(&limits);
    let listing = capture_listing_with_retry(&capability, &prefix, &mut budget)?;
    let revision = listing_revision(TREE_DOMAIN, &prefix, None, &listing);
    let binding = cursor_binding("tree", &prefix, None);
    let start = cursor_start(cursor.as_ref(), "tree", &binding, &revision, &listing)?;
    let page_limit = cmp::min(limit, limits.tree_page_entries) as usize;
    let end = cmp::min(start + page_limit, listing.len());
    let entries = listing[start..end]
        .iter()
        .map(LogicalEntry::api_entry)
        .collect();
    let next_cursor = if end < listing.len() {
        Some(encode_cursor(CursorPayload {
            kind: "tree".to_owned(),
            binding,
            revision: revision.as_str().to_owned(),
            last_path: listing[end - 1].path.clone(),
        })?)
    } else {
        None
    };
    Ok(FilePage {
        api_version: ApiVersion,
        workspace_id,
        revision,
        entries,
        next_cursor,
    })
}

fn search_sync(
    capability: WorkspaceCapability,
    _permit: ReviewPermit,
    limits: ReviewLimits,
    input: SearchInput,
) -> Result<FileSearchPage, ApiError> {
    let normalized_query = input.query.to_lowercase();
    let mut budget = RevisionBudget::new(&limits);
    let listing = capture_listing_with_retry(&capability, &input.prefix, &mut budget)?;
    let revision = listing_revision(
        SEARCH_DOMAIN,
        &input.prefix,
        Some(&normalized_query),
        &listing,
    );
    let binding = cursor_binding("search", &input.prefix, Some(&normalized_query));
    let start = cursor_start(
        input.cursor.as_ref(),
        "search",
        &binding,
        &revision,
        &listing,
    )?;
    let match_limit = cmp::min(input.limit, limits.search_page_matches) as usize;
    let scan_end = cmp::min(start + limits.search_scan_entries as usize, listing.len());
    let mut matches = Vec::new();
    let mut scanned_end = start;
    for entry in &listing[start..scan_end] {
        scanned_end += 1;
        let ranges = path_match_ranges(&entry.path, &normalized_query);
        if !ranges.is_empty() {
            matches.push(SearchMatch {
                entry: entry.api_entry(),
                path_match_ranges: ranges,
            });
            if matches.len() == match_limit {
                break;
            }
        }
    }
    let next_cursor = if scanned_end < listing.len() {
        Some(encode_cursor(CursorPayload {
            kind: "search".to_owned(),
            binding,
            revision: revision.as_str().to_owned(),
            last_path: listing[scanned_end - 1].path.clone(),
        })?)
    } else {
        None
    };
    Ok(FileSearchPage {
        api_version: ApiVersion,
        workspace_id: input.workspace_id,
        revision,
        matches,
        next_cursor,
    })
}

struct SearchInput {
    workspace_id: WorkspaceId,
    prefix: String,
    query: String,
    cursor: Option<PageCursor>,
    limit: u16,
}

fn content_sync(
    capability: WorkspaceCapability,
    _permit: ReviewPermit,
    limits: ReviewLimits,
    workspace_id: WorkspaceId,
    path: String,
    start_line: u32,
    end_line: u32,
) -> Result<FileContent, ApiError> {
    let mut budget = RevisionBudget::new(&limits);
    let capture =
        capture_file_with_retry(&capability, &path, &mut budget, CapturePurpose::File, true)?;
    if capture.binary {
        return Ok(FileContent {
            api_version: ApiVersion,
            workspace_id,
            path,
            revision: capture.revision,
            start_line,
            end_line: start_line,
            total_lines: None,
            text: None,
            binary: true,
            truncated: false,
            next_start_line: None,
        });
    }
    let text = String::from_utf8(capture.bytes)
        .map_err(|_| internal_error("validated review text is not UTF-8"))?;
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = normalized.split('\n').collect::<Vec<_>>();
    if normalized.ends_with('\n') {
        lines.pop();
    }
    if lines.is_empty() || start_line as usize > lines.len() {
        return Err(invalid_request(
            "review file line range is outside the file",
        ));
    }
    let configured_end = start_line
        .saturating_add(limits.file_lines.saturating_sub(1))
        .min(end_line)
        .min(lines.len() as u32);
    let first = start_line as usize - 1;
    let mut selected_end = configured_end as usize;
    let mut selected = lines[first..selected_end].join("\n");
    while selected.len() > limits.file_bytes && selected_end > first + 1 {
        selected_end -= 1;
        selected = lines[first..selected_end].join("\n");
    }
    if selected.len() > limits.file_bytes {
        return Err(payload_too_large(
            "review file response exceeds its byte limit",
        ));
    }
    let actual_end = selected_end as u32;
    let truncated = actual_end < lines.len() as u32;
    Ok(FileContent {
        api_version: ApiVersion,
        workspace_id,
        path,
        revision: capture.revision,
        start_line,
        end_line: actual_end,
        total_lines: (!truncated).then_some(lines.len() as u32),
        text: Some(selected),
        binary: false,
        truncated,
        next_start_line: truncated.then_some(actual_end + 1),
    })
}

#[derive(Debug, Clone)]
struct LogicalEntry {
    path: String,
    name: String,
    kind: FileKind,
    size_bytes: Option<u64>,
    binary: bool,
    revision: Option<RevisionToken>,
}

impl LogicalEntry {
    fn api_entry(&self) -> FileEntry {
        FileEntry {
            path: self.path.clone(),
            name: self.name.clone(),
            kind: self.kind,
            size_bytes: self.size_bytes,
            binary: self.binary,
            revision: self.revision.clone(),
            change: None,
        }
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let kind = match self.kind {
            FileKind::File => b"file".as_slice(),
            FileKind::Directory => b"directory".as_slice(),
        };
        let revision = self
            .revision
            .as_ref()
            .map_or(b"directory".as_slice(), |token| token.as_str().as_bytes());
        encode_fields(&[(1, self.path.as_bytes()), (2, kind), (3, revision)])
    }
}

fn capture_listing_with_retry(
    capability: &WorkspaceCapability,
    prefix: &str,
    budget: &mut RevisionBudget,
) -> Result<Vec<LogicalEntry>, ApiError> {
    loop {
        match capture_listing(capability, prefix, budget) {
            Ok(listing) => return Ok(listing),
            Err(CaptureError::Api(error)) => return Err(error),
            Err(CaptureError::Changed) => budget.begin_retry()?,
        }
    }
}

fn capture_listing(
    capability: &WorkspaceCapability,
    prefix: &str,
    budget: &mut RevisionBudget,
) -> Result<Vec<LogicalEntry>, CaptureError> {
    let mut entries = Vec::new();
    let mut pending = vec![prefix.to_owned()];
    while let Some(directory_path) = pending.pop() {
        budget.check_deadline()?;
        let normalized = if directory_path.is_empty() {
            None
        } else {
            Some(
                capability
                    .resolve(&directory_path)
                    .map_err(redact_platform_error)?,
            )
        };
        let directory = match normalized.as_ref() {
            Some(path) => capability.open_dir(path),
            None => capability.open_root(),
        }
        .map_err(redact_platform_error)?;
        let before = directory
            .dir_metadata()
            .map_err(|_| CaptureError::Changed)?;
        let reader = match normalized.as_ref() {
            Some(path) => capability.read_dir(path),
            None => capability.read_root(),
        }
        .map_err(redact_platform_error)?;
        for child in reader {
            let child = child.map_err(|_| CaptureError::Changed)?;
            budget.debit_listing(1, 0)?;
            let Some(name) = child.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let path = if directory_path.is_empty() {
                name.clone()
            } else {
                format!("{directory_path}/{name}")
            };
            capability.resolve(&path).map_err(redact_platform_error)?;
            let file_type = child.file_type().map_err(|_| CaptureError::Changed)?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                let entry = LogicalEntry {
                    path: path.clone(),
                    name,
                    kind: FileKind::Directory,
                    size_bytes: None,
                    binary: false,
                    revision: None,
                };
                budget.debit_listing(0, entry.canonical_bytes().len() as u64)?;
                entries.push(entry);
                pending.push(path);
            } else if file_type.is_file() {
                let capture =
                    capture_file(capability, &path, budget, CapturePurpose::Listing, false)?;
                let entry = LogicalEntry {
                    path,
                    name,
                    kind: FileKind::File,
                    size_bytes: Some(capture.size_bytes),
                    binary: capture.binary,
                    revision: Some(capture.revision),
                };
                budget.debit_listing(0, entry.canonical_bytes().len() as u64)?;
                entries.push(entry);
            }
        }
        let after_directory = match normalized.as_ref() {
            Some(path) => capability.open_dir(path),
            None => capability.open_root(),
        }
        .map_err(|_| CaptureError::Changed)?;
        let after = after_directory
            .dir_metadata()
            .map_err(|_| CaptureError::Changed)?;
        if !same_identity(&before, &after) || before.modified().ok() != after.modified().ok() {
            return Err(CaptureError::Changed);
        }
    }
    entries.sort_by(|left, right| left.path.as_bytes().cmp(right.path.as_bytes()));
    Ok(entries)
}

#[derive(Debug, Clone, Copy)]
enum CapturePurpose {
    File,
    Listing,
}

struct FileCapture {
    revision: RevisionToken,
    size_bytes: u64,
    binary: bool,
    bytes: Vec<u8>,
}

fn capture_file_with_retry(
    capability: &WorkspaceCapability,
    path: &str,
    budget: &mut RevisionBudget,
    purpose: CapturePurpose,
    retain: bool,
) -> Result<FileCapture, ApiError> {
    loop {
        match capture_file(capability, path, budget, purpose, retain) {
            Ok(capture) => return Ok(capture),
            Err(CaptureError::Api(error)) => return Err(error),
            Err(CaptureError::Changed) => budget.begin_retry()?,
        }
    }
}

fn capture_file(
    capability: &WorkspaceCapability,
    path: &str,
    budget: &mut RevisionBudget,
    purpose: CapturePurpose,
    retain: bool,
) -> Result<FileCapture, CaptureError> {
    let normalized = capability.resolve(path).map_err(redact_platform_error)?;
    let mut file = capability
        .open_file(&normalized)
        .map_err(redact_platform_error)?;
    let before = file.metadata().map_err(|_| CaptureError::Changed)?;
    let expected_size = before.len();
    let mut hasher = Sha256::new();
    hasher.update(FILE_DOMAIN);
    hasher.update(1_u32.to_be_bytes());
    hasher.update([1]);
    hasher.update(expected_size.to_be_bytes());
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; READ_BUFFER_BYTES];
    let mut total = 0_u64;
    let mut binary = false;
    let mut utf8 = Utf8State::default();
    loop {
        budget.check_deadline()?;
        let read = file
            .read(&mut buffer)
            .map_err(|_| unavailable("review file cannot be read"))?;
        if read == 0 {
            break;
        }
        match purpose {
            CapturePurpose::File => budget.debit_file_bytes(read as u64)?,
            CapturePurpose::Listing => budget.debit_listing(0, read as u64)?,
        }
        let chunk = &buffer[..read];
        total += read as u64;
        binary |= chunk.contains(&0);
        utf8.push(chunk);
        hasher.update(chunk);
        if retain {
            bytes.extend_from_slice(chunk);
        }
    }
    binary |= !utf8.finish();
    let after = file.metadata().map_err(|_| CaptureError::Changed)?;
    let rebound = capability
        .open_file(&normalized)
        .map_err(|_| CaptureError::Changed)?;
    let rebound_metadata = rebound.metadata().map_err(|_| CaptureError::Changed)?;
    if total != expected_size
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || !same_identity(&before, &after)
        || !same_identity(&after, &rebound_metadata)
    {
        return Err(CaptureError::Changed);
    }
    Ok(FileCapture {
        revision: revision_from_digest(hasher.finalize()),
        size_bytes: total,
        binary,
        bytes,
    })
}

#[derive(Default)]
struct Utf8State {
    tail: Vec<u8>,
    invalid: bool,
}

impl Utf8State {
    fn push(&mut self, chunk: &[u8]) {
        if self.invalid {
            return;
        }
        let mut offset = 0;
        if !self.tail.is_empty() {
            let Some(sequence_len) = utf8_sequence_len(self.tail[0]) else {
                self.invalid = true;
                return;
            };
            let needed = sequence_len - self.tail.len();
            let taken = cmp::min(needed, chunk.len());
            self.tail.extend_from_slice(&chunk[..taken]);
            offset = taken;
            if self.tail.len() < sequence_len {
                return;
            }
            if std::str::from_utf8(&self.tail).is_err() {
                self.invalid = true;
                return;
            }
            self.tail.clear();
        }
        if let Err(error) = std::str::from_utf8(&chunk[offset..]) {
            if error.error_len().is_some() {
                self.invalid = true;
            } else {
                self.tail
                    .extend_from_slice(&chunk[offset + error.valid_up_to()..]);
            }
        }
    }

    fn finish(&self) -> bool {
        !self.invalid && self.tail.is_empty()
    }
}

fn utf8_sequence_len(first: u8) -> Option<usize> {
    match first {
        0x00..=0x7f => Some(1),
        0xc2..=0xdf => Some(2),
        0xe0..=0xef => Some(3),
        0xf0..=0xf4 => Some(4),
        _ => None,
    }
}

enum CaptureError {
    Api(ApiError),
    Changed,
}

impl From<ApiError> for CaptureError {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

fn listing_revision(
    domain: &[u8],
    prefix: &str,
    query: Option<&str>,
    entries: &[LogicalEntry],
) -> RevisionToken {
    let fixed_fields = if query.is_some() { 2 } else { 1 };
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update((entries.len() as u32 + fixed_fields).to_be_bytes());
    update_field(&mut hasher, 1, prefix.as_bytes());
    if let Some(query) = query {
        update_field(&mut hasher, 2, query.as_bytes());
    }
    for entry in entries {
        update_field(&mut hasher, 3, &entry.canonical_bytes());
    }
    revision_from_digest(hasher.finalize())
}

fn encode_fields(fields: &[(u8, &[u8])]) -> Vec<u8> {
    let capacity = fields
        .iter()
        .map(|(_, value)| 1 + 8 + value.len())
        .sum::<usize>()
        + 4;
    let mut encoded = Vec::with_capacity(capacity);
    encoded.extend_from_slice(&(fields.len() as u32).to_be_bytes());
    for (tag, value) in fields {
        encoded.push(*tag);
        encoded.extend_from_slice(&(value.len() as u64).to_be_bytes());
        encoded.extend_from_slice(value);
    }
    encoded
}

fn update_field(hasher: &mut Sha256, tag: u8, value: &[u8]) {
    hasher.update([tag]);
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn revision_from_digest(digest: impl AsRef<[u8]>) -> RevisionToken {
    let value = digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    RevisionToken::parse(value).expect("SHA-256 is a canonical Review revision")
}

#[derive(Debug, Serialize, Deserialize)]
struct CursorPayload {
    kind: String,
    binding: String,
    revision: String,
    last_path: String,
}

fn cursor_binding(kind: &str, prefix: &str, query: Option<&str>) -> String {
    let mut fields = vec![(1, kind.as_bytes()), (2, prefix.as_bytes())];
    if let Some(query) = query {
        fields.push((3, query.as_bytes()));
    }
    let mut hasher = Sha256::new();
    hasher.update(CURSOR_DOMAIN);
    hasher.update((fields.len() as u32).to_be_bytes());
    for (tag, value) in fields {
        update_field(&mut hasher, tag, value);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn cursor_start(
    cursor: Option<&PageCursor>,
    kind: &str,
    binding: &str,
    revision: &RevisionToken,
    listing: &[LogicalEntry],
) -> Result<usize, ApiError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let payload = decode_cursor(cursor)?;
    if payload.kind != kind || payload.binding != binding {
        return Err(invalid_request(
            "review page cursor does not match the request",
        ));
    }
    if payload.revision != revision.as_str() {
        return Err(outdated("review page cursor revision is outdated"));
    }
    let start = listing.partition_point(|entry| entry.path <= payload.last_path);
    if start == 0 || listing[start - 1].path != payload.last_path {
        return Err(outdated("review page cursor position is outdated"));
    }
    Ok(start)
}

fn encode_cursor(payload: CursorPayload) -> Result<PageCursor, ApiError> {
    let bytes = serde_json::to_vec(&payload)
        .map_err(|_| internal_error("review page cursor cannot be encoded"))?;
    PageCursor::try_new(base64url_encode(&bytes))
        .map_err(|_| internal_error("review page cursor exceeds its wire limit"))
}

fn decode_cursor(cursor: &PageCursor) -> Result<CursorPayload, ApiError> {
    let bytes = base64url_decode(cursor.as_str())?;
    serde_json::from_slice(&bytes).map_err(|_| invalid_request("review page cursor is malformed"))
}

fn base64url_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        encoded.push(ALPHABET[((value >> 18) & 0x3f) as usize] as char);
        encoded.push(ALPHABET[((value >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(ALPHABET[((value >> 6) & 0x3f) as usize] as char);
        }
        if chunk.len() > 2 {
            encoded.push(ALPHABET[(value & 0x3f) as usize] as char);
        }
    }
    encoded
}

fn base64url_decode(value: &str) -> Result<Vec<u8>, ApiError> {
    if value.is_empty() || value.len() % 4 == 1 {
        return Err(invalid_request("review page cursor is malformed"));
    }
    let mut decoded = Vec::with_capacity(value.len() / 4 * 3);
    for chunk in value.as_bytes().chunks(4) {
        let mut bits = 0_u32;
        for (index, byte) in chunk.iter().enumerate() {
            bits |= u32::from(base64url_value(*byte)?) << (18 - index * 6);
        }
        decoded.push((bits >> 16) as u8);
        if chunk.len() > 2 {
            decoded.push((bits >> 8) as u8);
        }
        if chunk.len() > 3 {
            decoded.push(bits as u8);
        }
    }
    Ok(decoded)
}

fn base64url_value(byte: u8) -> Result<u8, ApiError> {
    match byte {
        b'A'..=b'Z' => Ok(byte - b'A'),
        b'a'..=b'z' => Ok(byte - b'a' + 26),
        b'0'..=b'9' => Ok(byte - b'0' + 52),
        b'-' => Ok(62),
        b'_' => Ok(63),
        _ => Err(invalid_request("review page cursor is malformed")),
    }
}

fn path_match_ranges(path: &str, normalized_query: &str) -> Vec<TextRange> {
    let query = normalized_query.chars().collect::<Vec<_>>();
    let mut folded = Vec::new();
    let mut original_indices = Vec::new();
    for (index, character) in path.chars().enumerate() {
        for lowered in character.to_lowercase() {
            folded.push(lowered);
            original_indices.push(index as u32);
        }
    }
    if query.is_empty() || query.len() > folded.len() {
        return Vec::new();
    }
    folded
        .windows(query.len())
        .enumerate()
        .filter(|(_, window)| *window == query.as_slice())
        .map(|(start, _)| TextRange {
            start: original_indices[start],
            end: original_indices[start + query.len() - 1] + 1,
        })
        .collect()
}

fn validate_limit(limit: u16) -> Result<(), ApiError> {
    if limit == 0 {
        Err(invalid_request("review page limit must be positive"))
    } else {
        Ok(())
    }
}

fn validate_path(path: &str, limits: &ReviewLimits) -> Result<(), ApiError> {
    if path.is_empty()
        || path.len() > limits.path_bytes
        || path.split('/').count() > limits.path_components
    {
        Err(invalid_request("review workspace path is invalid"))
    } else {
        Ok(())
    }
}

fn validate_prefix(prefix: &str, limits: &ReviewLimits) -> Result<(), ApiError> {
    if prefix.is_empty() {
        Ok(())
    } else {
        validate_path(prefix, limits)
    }
}

#[cfg(unix)]
fn same_identity(left: &cap_std::fs::Metadata, right: &cap_std::fs::Metadata) -> bool {
    use cap_std::fs::MetadataExt;

    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(windows)]
fn same_identity(left: &cap_std::fs::Metadata, right: &cap_std::fs::Metadata) -> bool {
    use cap_std::fs::MetadataExt;

    left.volume_serial_number() == right.volume_serial_number()
        && left.file_index() == right.file_index()
}

#[cfg(not(any(unix, windows)))]
fn same_identity(left: &cap_std::fs::Metadata, right: &cap_std::fs::Metadata) -> bool {
    left.len() == right.len() && left.modified().ok() == right.modified().ok()
}

fn redact_platform_error(error: ApiError) -> ApiError {
    match error.code() {
        ApiErrorCode::InvalidRequest => invalid_request("review workspace path is invalid"),
        ApiErrorCode::WorkspaceNotFound => ApiError::new(
            ApiErrorCode::WorkspaceNotFound,
            "review workspace is unavailable",
            "review-files",
        ),
        ApiErrorCode::WorkspaceUnavailable => ApiError::new(
            ApiErrorCode::FileNotFound,
            "review workspace entry was not found",
            "review-files",
        ),
        _ => internal_error("review workspace operation failed"),
    }
}

fn redact_capability_error(error: ApiError) -> ApiError {
    match error.code() {
        ApiErrorCode::WorkspaceNotFound => ApiError::new(
            ApiErrorCode::WorkspaceNotFound,
            "review workspace is unavailable",
            "review-files",
        ),
        ApiErrorCode::WorkspaceUnavailable => unavailable("review workspace is unavailable"),
        _ => internal_error("review workspace operation failed"),
    }
}

fn invalid_request(message: &'static str) -> ApiError {
    ApiError::new(ApiErrorCode::InvalidRequest, message, "review-files")
}

fn payload_too_large(message: &'static str) -> ApiError {
    ApiError::new(ApiErrorCode::PayloadTooLarge, message, "review-files")
}

fn outdated(message: &'static str) -> ApiError {
    ApiError::new(ApiErrorCode::Outdated, message, "review-files")
}

fn unavailable(message: &'static str) -> ApiError {
    ApiError::new(ApiErrorCode::WorkspaceUnavailable, message, "review-files")
}

fn internal_error(message: &'static str) -> ApiError {
    ApiError::new(ApiErrorCode::Internal, message, "review-files")
}
