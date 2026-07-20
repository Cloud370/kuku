//! Provides identity-bound, bounded Git review snapshots and diffs.

use std::collections::BTreeMap;
use std::future::Future;
use std::io::Read;
use std::pin::Pin;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::api::{
    ApiError, ApiErrorCode, ApiVersion, ChangeEntry, ChangeKind, ChangesAvailability, DiffDocument,
    DiffHunk, DiffLine, DiffLineKind, PageCursor, ReviewSnapshot, RevisionToken,
};
use crate::platform::{
    ProcessChunk, ProcessChunkSink, ProcessLimits, ProcessOutput, ProcessStream, RootCommand,
    WorkspaceCapability,
};
use crate::review::{ReviewLimits, RevisionBudget};

mod parser;
use parser::{
    index_by_path, lines_to_hunks, logical_index_records, logical_line_count, nul_strings,
    UnifiedParser,
};
pub(crate) use parser::{parse_numstat, parse_porcelain_v2};

const GIT_PREFIX: [&str; 4] = [
    "--no-pager",
    "--literal-pathspecs",
    "-c",
    "core.fsmonitor=false",
];
const TRACE_ID: &str = "review-git";

#[derive(Debug, Clone)]
struct CapturedState {
    revision: RevisionToken,
    paths: BTreeMap<String, RevisionToken>,
}

#[derive(Debug)]
enum GitProbe {
    Available,
    NotRepository,
    RootMismatch,
    Unavailable,
}

type CaptureHook = Arc<dyn Fn() + Send + Sync>;
type StableSnapshot = (CapturedState, Vec<ChangeEntry>);

/// Reads aggregate workspace changes through a Platform-owned capability.
#[derive(Clone)]
pub(crate) struct GitReviewService {
    capability: WorkspaceCapability,
    limits: ReviewLimits,
    capture_hook: Option<CaptureHook>,
}

impl GitReviewService {
    pub(crate) fn new(capability: WorkspaceCapability, limits: ReviewLimits) -> Self {
        Self {
            capability,
            limits,
            capture_hook: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_with_capture_hook(
        capability: WorkspaceCapability,
        limits: ReviewLimits,
        capture_hook: CaptureHook,
    ) -> Self {
        Self {
            capability,
            limits,
            capture_hook: Some(capture_hook),
        }
    }

    pub(crate) async fn snapshot(
        &self,
        cursor: Option<&PageCursor>,
        limit: u16,
    ) -> Result<ReviewSnapshot, ApiError> {
        let limit = self.valid_limit(limit)?;
        let probe = self.probe().await;
        if !matches!(probe, GitProbe::Available) {
            return Ok(self.unavailable_snapshot(probe));
        }
        let mut budget = RevisionBudget::new(&self.limits);
        let stable = match self.stable_snapshot(&mut budget).await {
            Ok(stable) => stable,
            Err(_) => return Ok(self.unavailable_snapshot(GitProbe::Unavailable)),
        };
        let offset = cursor.map_or(Ok(0), |cursor| {
            parse_cursor(cursor, "git-changes-v1", &stable.0.revision, None)
        })?;
        if offset > stable.1.len() {
            return Err(invalid_cursor());
        }
        let end = offset.saturating_add(limit).min(stable.1.len());
        let next_cursor = if end < stable.1.len() {
            Some(make_cursor(
                "git-changes-v1",
                &stable.0.revision,
                None,
                end,
            )?)
        } else {
            None
        };
        Ok(ReviewSnapshot {
            api_version: ApiVersion,
            workspace_id: self.capability.workspace_id().clone(),
            revision: stable.0.revision,
            availability: ChangesAvailability::Available,
            entries: stable.1[offset..end].to_vec(),
            next_cursor,
        })
    }

    pub(crate) async fn diff(
        &self,
        path: &str,
        revision: &RevisionToken,
        cursor: Option<&PageCursor>,
        limit: u16,
    ) -> Result<DiffDocument, ApiError> {
        let limit = self
            .valid_limit(limit)?
            .min(self.limits.diff_lines as usize);
        self.capability.resolve(path)?;
        if !matches!(self.probe().await, GitProbe::Available) {
            return Err(outdated());
        }
        let mut budget = RevisionBudget::new(&self.limits);
        for attempt in 0..2 {
            let before = self.stable_snapshot(&mut budget).await?;
            let entry = before
                .1
                .iter()
                .find(|entry| entry.path == path)
                .ok_or_else(outdated)?;
            if &entry.revision != revision {
                return Err(outdated());
            }
            let path_token = token_for(b"kuku.review.git.cursor-path.v1", path.as_bytes());
            let offset = cursor.map_or(Ok(0), |cursor| {
                parse_cursor(cursor, "git-diff-v1", revision, Some(&path_token))
            })?;
            let page = if entry.kind == ChangeKind::Untracked {
                self.untracked_diff(path, revision, offset, limit, &mut budget)?
            } else {
                self.tracked_diff(entry, revision, offset, limit, &mut budget)
                    .await?
            };
            let after = self.capture_exact_git_state(&mut budget).await?;
            if before.0.revision == after.revision {
                return Ok(page);
            }
            if attempt == 0 {
                budget.begin_retry()?;
            }
        }
        Err(outdated())
    }

    async fn probe(&self) -> GitProbe {
        let output = match self
            .run(&["rev-parse", "--path-format=absolute", "--show-toplevel"])
            .await
        {
            Ok(output) => output,
            Err(_) => return GitProbe::Unavailable,
        };
        if !output.status().success() {
            return GitProbe::NotRepository;
        }
        if !self.capability.reported_root_is_self(&output) {
            return GitProbe::RootMismatch;
        }
        GitProbe::Available
    }

    async fn stable_snapshot(
        &self,
        budget: &mut RevisionBudget,
    ) -> Result<StableSnapshot, ApiError> {
        for attempt in 0..2 {
            let before = self.capture_exact_git_state(budget).await?;
            let status = self
                .required(&["status", "--porcelain=v2", "-z", "--untracked-files=all"])
                .await?;
            if let Some(hook) = &self.capture_hook {
                hook();
            }
            let numstat = self
                .required(&[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--numstat",
                    "-z",
                    "HEAD",
                    "--",
                ])
                .await?;
            budget.debit_git(0, (status.len() + numstat.len()) as u64)?;
            let after = self.capture_exact_git_state(budget).await?;
            if before.revision == after.revision {
                let entries = self.build_entries(&status, &numstat, &after, budget)?;
                return Ok((after, entries));
            }
            if attempt == 0 {
                budget.begin_retry()?;
            }
        }
        Err(outdated())
    }

    async fn capture_exact_git_state(
        &self,
        budget: &mut RevisionBudget,
    ) -> Result<CapturedState, ApiError> {
        budget.check_deadline()?;
        let head = self
            .optional(&["rev-parse", "--verify", "HEAD^{commit}"])
            .await?;
        let tree = self
            .optional(&["rev-parse", "--verify", "HEAD^{tree}"])
            .await?;
        let index = self
            .required(&["ls-files", "--stage", "--debug", "-z", "--"])
            .await?;
        let listed = self
            .required(&[
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
                "--",
            ])
            .await?;
        let logical_index = logical_index_records(&index)?;
        let mut aggregate = FramedHash::new(b"kuku.review.git.aggregate.v1");
        aggregate.field(head.as_deref().unwrap_or(b"unborn"));
        aggregate.field(tree.as_deref().unwrap_or(b"unborn-tree"));
        for record in &logical_index {
            aggregate.field(record);
        }
        let index_by_path = index_by_path(&logical_index)?;
        let mut paths = BTreeMap::new();
        let mut names = nul_strings(&listed)?;
        names.sort();
        names.dedup();
        for path in names {
            let mut path_hash = FramedHash::new(b"kuku.review.git.change.v1");
            path_hash.field(path.as_bytes());
            if let Some(index_record) = index_by_path.get(path.as_str()) {
                path_hash.field(index_record);
            } else {
                path_hash.field(b"untracked");
            }
            let relative = self.capability.resolve(&path)?;
            match self.capability.open_file(&relative) {
                Ok(mut file) => {
                    let mut buffer = [0_u8; 64 * 1024];
                    loop {
                        let read = file
                            .read(&mut buffer)
                            .map_err(|_| workspace_unavailable())?;
                        if read == 0 {
                            break;
                        }
                        budget.debit_git(0, read as u64)?;
                        path_hash.bytes(&buffer[..read]);
                    }
                }
                Err(_) => match self.capability.read_link_target(&relative) {
                    Ok(target) => {
                        path_hash.field(b"symlink-target");
                        budget.debit_git(0, target.len() as u64)?;
                        path_hash.bytes(&target);
                    }
                    Err(_) => path_hash.field(b"missing-or-nonregular"),
                },
            }
            let token = path_hash.finish();
            aggregate.field(token.as_str().as_bytes());
            paths.insert(path, token);
            budget.debit_git(1, 0)?;
        }
        budget.debit_git(
            0,
            (head.as_ref().map_or(0, Vec::len)
                + tree.as_ref().map_or(0, Vec::len)
                + index.len()
                + listed.len()) as u64,
        )?;
        Ok(CapturedState {
            revision: aggregate.finish(),
            paths,
        })
    }

    fn build_entries(
        &self,
        status: &[u8],
        numstat: &[u8],
        state: &CapturedState,
        budget: &mut RevisionBudget,
    ) -> Result<Vec<ChangeEntry>, ApiError> {
        let changes = parse_porcelain_v2(status)?;
        let stats = parse_numstat(numstat)?;
        let mut entries = Vec::with_capacity(changes.len());
        for change in changes {
            let (additions, deletions, binary) = if change.kind == ChangeKind::Untracked {
                self.untracked_stat(&change.path, budget)?
            } else {
                let counts = stats
                    .get(&change.path)
                    .copied()
                    .unwrap_or((Some(0), Some(0)));
                (counts.0, counts.1, counts.0.is_none() || counts.1.is_none())
            };
            let content = state.paths.get(&change.path).cloned().unwrap_or_else(|| {
                token_for(b"kuku.review.git.missing.v1", change.path.as_bytes())
            });
            let mut hash = FramedHash::new(b"kuku.review.git.entry.v1");
            hash.field(content.as_str().as_bytes());
            hash.field(format!("{:?}", change.kind).as_bytes());
            hash.field(&[change.staged as u8, change.worktree as u8]);
            if let Some(old_path) = &change.old_path {
                hash.field(old_path.as_bytes());
            }
            if let Some(value) = additions {
                hash.field(&value.to_be_bytes());
            }
            if let Some(value) = deletions {
                hash.field(&value.to_be_bytes());
            }
            entries.push(ChangeEntry {
                path: change.path,
                old_path: change.old_path,
                kind: change.kind,
                staged: change.staged,
                worktree: change.worktree,
                binary,
                additions,
                deletions,
                revision: hash.finish(),
            });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(entries)
    }

    fn untracked_stat(
        &self,
        path: &str,
        budget: &mut RevisionBudget,
    ) -> Result<(Option<u32>, Option<u32>, bool), ApiError> {
        let bytes = self.read_file(path, budget)?;
        if bytes.contains(&0) {
            return Ok((None, None, true));
        }
        let lines = logical_line_count(&bytes)?;
        Ok((Some(lines), Some(0), false))
    }

    fn untracked_diff(
        &self,
        path: &str,
        revision: &RevisionToken,
        offset: usize,
        limit: usize,
        budget: &mut RevisionBudget,
    ) -> Result<DiffDocument, ApiError> {
        let bytes = self.read_file(path, budget)?;
        if bytes.contains(&0) {
            return Ok(self.diff_document(path, revision, None, true, Vec::new(), None));
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| payload_too_large())?;
        let mut lines = text
            .split_terminator('\n')
            .enumerate()
            .map(|(index, text)| DiffLine {
                kind: DiffLineKind::Addition,
                old_line: None,
                new_line: Some((index + 1) as u32),
                text: text.strip_suffix('\r').unwrap_or(text).to_owned(),
            })
            .collect::<Vec<_>>();
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            lines.push(DiffLine {
                kind: DiffLineKind::NoNewlineMarker,
                old_line: None,
                new_line: None,
                text: "No newline at end of file".to_owned(),
            });
        }
        self.page_diff(path, revision, lines, offset, limit)
    }

    async fn tracked_diff(
        &self,
        entry: &ChangeEntry,
        revision: &RevisionToken,
        offset: usize,
        limit: usize,
        budget: &mut RevisionBudget,
    ) -> Result<DiffDocument, ApiError> {
        let command = git_command(&[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--binary",
            "--no-color",
            "--unified=80",
            "HEAD",
            "--",
            &entry.path,
        ]);
        let stream_limit = usize::try_from(self.limits.revision_git_hash_bytes)
            .unwrap_or(usize::MAX)
            .min(1024 * 1024 * 1024);
        let process_limits = ProcessLimits::new(self.limits.git_deadline, stream_limit)?;
        let mut sink = DiffSink::new(
            offset,
            limit,
            self.limits.diff_bytes,
            self.limits.git_stream_bytes,
        );
        let status = self
            .capability
            .stream_at_root(command, process_limits, &mut sink)
            .await?;
        if !status.success() {
            return Err(workspace_unavailable());
        }
        budget.debit_git(0, sink.streamed as u64)?;
        let parsed = sink.finish()?;
        let next = if parsed.has_more {
            Some(make_cursor(
                "git-diff-v1",
                revision,
                Some(&token_for(
                    b"kuku.review.git.cursor-path.v1",
                    entry.path.as_bytes(),
                )),
                offset + parsed.lines.len(),
            )?)
        } else {
            None
        };
        let hunks = lines_to_hunks(parsed.lines);
        Ok(self.diff_document(
            &entry.path,
            revision,
            entry.old_path.clone(),
            parsed.binary,
            hunks,
            next,
        ))
    }

    fn page_diff(
        &self,
        path: &str,
        revision: &RevisionToken,
        lines: Vec<DiffLine>,
        offset: usize,
        limit: usize,
    ) -> Result<DiffDocument, ApiError> {
        if offset > lines.len() {
            return Err(invalid_cursor());
        }
        let end = offset.saturating_add(limit).min(lines.len());
        let next = if end < lines.len() {
            Some(make_cursor(
                "git-diff-v1",
                revision,
                Some(&token_for(
                    b"kuku.review.git.cursor-path.v1",
                    path.as_bytes(),
                )),
                end,
            )?)
        } else {
            None
        };
        Ok(self.diff_document(
            path,
            revision,
            None,
            false,
            lines_to_hunks(lines[offset..end].to_vec()),
            next,
        ))
    }

    fn diff_document(
        &self,
        path: &str,
        revision: &RevisionToken,
        old_path: Option<String>,
        binary: bool,
        hunks: Vec<DiffHunk>,
        next_cursor: Option<PageCursor>,
    ) -> DiffDocument {
        DiffDocument {
            api_version: ApiVersion,
            workspace_id: self.capability.workspace_id().clone(),
            path: path.to_owned(),
            old_path,
            revision: revision.clone(),
            binary,
            hunks,
            truncated: next_cursor.is_some(),
            next_cursor,
        }
    }

    fn read_file(&self, path: &str, budget: &mut RevisionBudget) -> Result<Vec<u8>, ApiError> {
        let relative = self.capability.resolve(path)?;
        let bytes = match self.capability.open_file(&relative) {
            Ok(mut file) => {
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)
                    .map_err(|_| workspace_unavailable())?;
                bytes
            }
            Err(_) => self.capability.read_link_target(&relative)?,
        };
        budget.debit_git(0, bytes.len() as u64)?;
        Ok(bytes)
    }

    async fn run(&self, args: &[&str]) -> Result<ProcessOutput, ApiError> {
        let limits = ProcessLimits::new(self.limits.git_deadline, self.limits.git_stream_bytes)?;
        self.capability.run_at_root(git_command(args), limits).await
    }

    async fn required(&self, args: &[&str]) -> Result<Vec<u8>, ApiError> {
        let output = self.run(args).await?;
        if !output.status().success() {
            return Err(workspace_unavailable());
        }
        Ok(output.stdout().to_vec())
    }

    async fn optional(&self, args: &[&str]) -> Result<Option<Vec<u8>>, ApiError> {
        let output = self.run(args).await?;
        if output.status().success() {
            Ok(Some(output.stdout().to_vec()))
        } else {
            Ok(None)
        }
    }

    fn valid_limit(&self, limit: u16) -> Result<usize, ApiError> {
        if limit == 0 {
            Err(error(
                ApiErrorCode::InvalidRequest,
                "review page limit must be positive",
            ))
        } else {
            Ok(limit as usize)
        }
    }

    fn unavailable_snapshot(&self, probe: GitProbe) -> ReviewSnapshot {
        let availability = match probe {
            GitProbe::Available | GitProbe::Unavailable => ChangesAvailability::GitUnavailable,
            GitProbe::NotRepository => ChangesAvailability::NotGitRepository,
            GitProbe::RootMismatch => ChangesAvailability::WorkspaceRootMismatch,
        };
        ReviewSnapshot {
            api_version: ApiVersion,
            workspace_id: self.capability.workspace_id().clone(),
            revision: token_for(
                b"kuku.review.git.availability.v1",
                format!("{availability:?}").as_bytes(),
            ),
            availability,
            entries: Vec::new(),
            next_cursor: None,
        }
    }
}

fn git_command(args: &[&str]) -> RootCommand {
    let mut command = RootCommand::new("git").args(GIT_PREFIX);
    command = command.args(args.iter().copied());
    let null_config = if cfg!(windows) { "NUL" } else { "/dev/null" };
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", null_config)
        .env("LC_ALL", "C")
        .env("LANG", "C")
}

#[derive(Debug)]
struct StreamedDiff {
    binary: bool,
    lines: Vec<DiffLine>,
    has_more: bool,
}

#[derive(Debug)]
struct DiffSink {
    parser: UnifiedParser,
    pending: Vec<u8>,
    offset: usize,
    limit: usize,
    max_bytes: usize,
    max_stderr: usize,
    stderr_bytes: usize,
    retained_bytes: usize,
    seen_lines: usize,
    streamed: usize,
    has_more: bool,
}

impl DiffSink {
    fn new(offset: usize, limit: usize, max_bytes: usize, max_stderr: usize) -> Self {
        Self {
            parser: UnifiedParser::default(),
            pending: Vec::new(),
            offset,
            limit,
            max_bytes,
            max_stderr,
            stderr_bytes: 0,
            retained_bytes: 0,
            seen_lines: 0,
            streamed: 0,
            has_more: false,
        }
    }

    fn consume(&mut self, bytes: &[u8]) -> Result<(), ApiError> {
        self.streamed = self.streamed.saturating_add(bytes.len());
        self.pending.extend_from_slice(bytes);
        while let Some(index) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line = self.pending.drain(..=index).collect::<Vec<_>>();
            self.consume_line(&line)?;
        }
        if self.pending.len() > self.max_bytes {
            return Err(payload_too_large());
        }
        Ok(())
    }

    fn consume_line(&mut self, line: &[u8]) -> Result<(), ApiError> {
        let before = self.parser.lines.len();
        self.parser.push_line(line)?;
        if self.parser.lines.len() == before {
            return Ok(());
        }
        let parsed = self.parser.lines.pop().expect("parser appended one line");
        if self.seen_lines < self.offset {
            self.seen_lines += 1;
            return Ok(());
        }
        if self.parser.lines.len() >= self.limit
            || self.retained_bytes.saturating_add(parsed.text.len()) > self.max_bytes
        {
            self.has_more = true;
            self.seen_lines += 1;
            return Ok(());
        }
        self.retained_bytes += parsed.text.len();
        self.parser.lines.push(parsed);
        self.seen_lines += 1;
        Ok(())
    }

    fn finish(mut self) -> Result<StreamedDiff, ApiError> {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            self.consume_line(&pending)?;
        }
        Ok(StreamedDiff {
            binary: self.parser.binary,
            lines: self.parser.lines,
            has_more: self.has_more,
        })
    }
}

impl ProcessChunkSink for DiffSink {
    fn push<'a>(
        &'a mut self,
        chunk: ProcessChunk,
    ) -> Pin<Box<dyn Future<Output = Result<(), ApiError>> + Send + 'a>> {
        Box::pin(async move {
            match chunk.stream() {
                ProcessStream::Stdout => self.consume(chunk.bytes())?,
                ProcessStream::Stderr => {
                    self.stderr_bytes = self.stderr_bytes.saturating_add(chunk.bytes().len());
                    if self.stderr_bytes > self.max_stderr {
                        return Err(payload_too_large());
                    }
                }
            }
            Ok(())
        })
    }
}

#[derive(Debug)]
struct FramedHash(Sha256);

impl FramedHash {
    fn new(domain: &[u8]) -> Self {
        let mut hash = Sha256::new();
        hash.update((domain.len() as u64).to_be_bytes());
        hash.update(domain);
        Self(hash)
    }

    fn field(&mut self, bytes: &[u8]) {
        self.0.update((bytes.len() as u64).to_be_bytes());
        self.0.update(bytes);
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    fn finish(self) -> RevisionToken {
        RevisionToken::parse(format!("{:x}", self.0.finalize()))
            .expect("SHA-256 is a valid revision token")
    }
}

fn token_for(domain: &[u8], bytes: &[u8]) -> RevisionToken {
    let mut hash = FramedHash::new(domain);
    hash.field(bytes);
    hash.finish()
}

fn make_cursor(
    domain: &str,
    revision: &RevisionToken,
    path: Option<&RevisionToken>,
    offset: usize,
) -> Result<PageCursor, ApiError> {
    let value = match path {
        Some(path) => format!("{domain}:{}:{}:{offset}", revision.as_str(), path.as_str()),
        None => format!("{domain}:{}:{offset}", revision.as_str()),
    };
    PageCursor::try_new(value).map_err(|_| invalid_cursor())
}

fn parse_cursor(
    cursor: &PageCursor,
    domain: &str,
    revision: &RevisionToken,
    path: Option<&RevisionToken>,
) -> Result<usize, ApiError> {
    let fields = cursor.as_str().split(':').collect::<Vec<_>>();
    let expected = if path.is_some() { 4 } else { 3 };
    if fields.len() != expected || fields[0] != domain || fields[1] != revision.as_str() {
        return Err(invalid_cursor());
    }
    if let Some(path) = path {
        if fields[2] != path.as_str() {
            return Err(invalid_cursor());
        }
    }
    fields[expected - 1]
        .parse::<usize>()
        .map_err(|_| invalid_cursor())
}

fn invalid_cursor() -> ApiError {
    error(
        ApiErrorCode::InvalidRequest,
        "review cursor does not match content",
    )
}

fn invalid_git_output() -> ApiError {
    unavailable("Git returned malformed bounded output")
}

fn workspace_unavailable() -> ApiError {
    unavailable("workspace Git state is unavailable")
}

fn payload_too_large() -> ApiError {
    error(
        ApiErrorCode::PayloadTooLarge,
        "review Git budget is exhausted",
    )
}

fn outdated() -> ApiError {
    error(ApiErrorCode::Outdated, "review Git content is outdated")
}

fn unavailable(message: &str) -> ApiError {
    error(ApiErrorCode::WorkspaceUnavailable, message)
}

fn error(code: ApiErrorCode, message: &str) -> ApiError {
    ApiError::new(code, message, TRACE_ID)
}
