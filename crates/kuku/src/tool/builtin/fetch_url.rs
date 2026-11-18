use std::future::Future;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::sync::Notify;
use tokio_stream::StreamExt;

use crate::tool::ToolResultEnvelope;
use crate::{OutputReference, OutputStream};

use super::run_command::output::{framed_bytes, OutputLimits, OutputWriter, MAX_FRAME_BYTES};

const MAX_DOWNLOAD_BYTES: u64 = 50 * 1024 * 1024;
const MAX_URL_BYTES: usize = 8192;
const MAX_CONTENT_TYPE_CHARS: usize = 256;

pub(crate) struct FetchRetention<'a> {
    pub(crate) dir: &'a Path,
    pub(crate) id: &'a str,
    pub(crate) limits: OutputLimits,
}

pub(crate) async fn fetch_url(
    args: &Value,
    _workspace: &Path,
    retained: Option<FetchRetention<'_>>,
    cancel: &Notify,
) -> ToolResultEnvelope {
    match fetch_url_request(args) {
        Ok(url) => download(url, retained, cancel)
            .await
            .unwrap_or_else(|error| error),
        Err(error) => error,
    }
}

pub(super) fn validate_url(url: &str) -> Result<(), ToolResultEnvelope> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ToolResultEnvelope::error(
            "failed: invalid url protocol",
            "url must start with http:// or https://",
        ));
    }
    if url.len() > MAX_URL_BYTES {
        return Err(ToolResultEnvelope::error(
            "failed: url too long",
            format!("url exceeds {MAX_URL_BYTES} bytes"),
        ));
    }
    let parsed = url::Url::parse(url).map_err(|error| {
        ToolResultEnvelope::error(
            "failed: invalid url",
            format!("failed to parse URL: {error}"),
        )
    })?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(ToolResultEnvelope::error(
            "failed: url contains credentials",
            "url must not contain a username or password",
        ));
    }
    Ok(())
}

fn fetch_url_request(args: &Value) -> Result<&str, ToolResultEnvelope> {
    let url = args.get("url").and_then(Value::as_str).ok_or_else(|| {
        ToolResultEnvelope::error("failed: missing url", "fetch_url requires url")
    })?;
    validate_url(url)?;
    Ok(url)
}

struct DownloadHeaders {
    status: u16,
    content_type: String,
    content_type_truncated: bool,
    filename: String,
}

struct DownloadDigest {
    size: u64,
    sha256: String,
}

enum DownloadOutput<'a> {
    Retained {
        writer: Box<OutputWriter>,
        id: &'a str,
        pending: Vec<u8>,
    },
    Temporary {
        file: tokio::fs::File,
        filename: String,
        directory: tempfile::TempDir,
    },
}

enum DownloadLocation {
    Retained(OutputReference),
    Temporary(PathBuf),
}

impl<'a> DownloadOutput<'a> {
    async fn open(
        filename: &str,
        retained: Option<FetchRetention<'a>>,
    ) -> Result<Self, ToolResultEnvelope> {
        if let Some(retained) = retained {
            let writer = OutputWriter::create(retained.dir, retained.id, retained.limits)
                .await
                .map_err(retained_write_error)?;
            return Ok(Self::Retained {
                writer: Box::new(writer),
                id: retained.id,
                pending: Vec::with_capacity(MAX_FRAME_BYTES),
            });
        }
        let directory = tempfile::Builder::new()
            .prefix("kuku-fetch-")
            .tempdir()
            .map_err(temp_write_error)?;
        let path = directory.path().join(filename);
        let file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .await
            .map_err(temp_write_error)?;
        Ok(Self::Temporary {
            file,
            filename: filename.into(),
            directory,
        })
    }

    async fn write(&mut self, mut bytes: &[u8]) -> Result<(), ToolResultEnvelope> {
        match self {
            Self::Temporary { file, .. } => file.write_all(bytes).await.map_err(temp_write_error),
            Self::Retained {
                writer, pending, ..
            } => {
                while !bytes.is_empty() {
                    let (part, remaining) =
                        bytes.split_at(bytes.len().min(MAX_FRAME_BYTES - pending.len()));
                    pending.extend_from_slice(part);
                    if pending.len() == MAX_FRAME_BYTES {
                        write_frame(writer, pending).await?;
                        pending.clear();
                    }
                    bytes = remaining;
                }
                Ok(())
            }
        }
    }

    async fn finish(self) -> Result<DownloadLocation, ToolResultEnvelope> {
        match self {
            Self::Retained {
                mut writer,
                id,
                pending,
            } => {
                if let Err(error) = write_frame(&mut writer, &pending).await {
                    writer.close().await;
                    return Err(error);
                }
                let metadata = writer.finish(false).await.map_err(retained_write_error)?;
                Ok(DownloadLocation::Retained(OutputReference {
                    id: id.into(),
                    offset: 0,
                    bytes: metadata.stored_bytes,
                }))
            }
            Self::Temporary {
                mut file,
                filename,
                directory,
            } => {
                let flushed = file.flush().await;
                drop(file.into_std().await);
                flushed.map_err(temp_write_error)?;
                Ok(DownloadLocation::Temporary(directory.keep().join(filename)))
            }
        }
    }

    async fn close(self) {
        match self {
            Self::Retained { writer, .. } => writer.close().await,
            Self::Temporary {
                file, directory, ..
            } => {
                drop(file.into_std().await);
                drop(directory);
            }
        }
    }
}

async fn write_frame(writer: &mut OutputWriter, frame: &[u8]) -> Result<(), ToolResultEnvelope> {
    writer
        .write(OutputStream::Stdout, frame)
        .await
        .map_err(retained_write_error)?;
    if writer.quota_truncated() {
        return Err(quota_error());
    }
    Ok(())
}

async fn download(
    url: &str,
    retained: Option<FetchRetention<'_>>,
    cancel: &Notify,
) -> Result<ToolResultEnvelope, ToolResultEnvelope> {
    let client = crate::provider::http_client::fetch_client();
    let response = until_cancelled(cancel, client.get(url).send())
        .await?
        .map_err(|error| {
            ToolResultEnvelope::error(
                "failed: fetch error",
                format!("failed to fetch URL: {error}"),
            )
        })?;
    let quota = retained
        .as_ref()
        .map(|retained| retained.limits.max_output_bytes);
    if let Some(size) = response.content_length() {
        check_size(size, quota)?;
    }
    let headers = download_headers(url, &response);
    let mut output = DownloadOutput::open(&headers.filename, retained).await?;
    let digest = match transfer(response, &mut output, quota, cancel).await {
        Ok(digest) => digest,
        Err(error) => {
            output.close().await;
            return Err(error);
        }
    };
    let location = output.finish().await?;
    Ok(render_download(url, headers, digest, location))
}

async fn transfer(
    response: wreq::Response,
    output: &mut DownloadOutput<'_>,
    quota: Option<u64>,
    cancel: &Notify,
) -> Result<DownloadDigest, ToolResultEnvelope> {
    let mut stream = response.bytes_stream();
    let mut size = 0;
    let mut digest = Sha256::new();
    while let Some(chunk) = until_cancelled(cancel, stream.next()).await? {
        let chunk = chunk.map_err(|error| {
            ToolResultEnvelope::error(
                "failed: download error",
                format!("failed to read response body: {error}"),
            )
        })?;
        size += chunk.len() as u64;
        check_size(size, quota)?;
        until_cancelled(cancel, output.write(&chunk)).await??;
        digest.update(&chunk);
    }
    Ok(DownloadDigest {
        size,
        sha256: format!("{:x}", digest.finalize()),
    })
}

fn check_size(size: u64, quota: Option<u64>) -> Result<(), ToolResultEnvelope> {
    if size > MAX_DOWNLOAD_BYTES {
        return Err(size_error(size));
    }
    if quota.is_some_and(|quota| framed_bytes(size) > quota) {
        return Err(quota_error());
    }
    Ok(())
}

fn download_headers(url: &str, response: &wreq::Response) -> DownloadHeaders {
    let source_content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unknown");
    let content_type: String = source_content_type
        .chars()
        .take(MAX_CONTENT_TYPE_CHARS)
        .collect();
    DownloadHeaders {
        status: response.status().as_u16(),
        content_type_truncated: content_type.len() < source_content_type.len(),
        content_type,
        filename: extract_filename(url, response.headers()),
    }
}

fn render_download(
    url: &str,
    headers: DownloadHeaders,
    digest: DownloadDigest,
    location: DownloadLocation,
) -> ToolResultEnvelope {
    let mut facts = json!({
        "kind": "fetch_url", "url": url, "status": headers.status,
        "content_type": headers.content_type, "content_type_truncated": headers.content_type_truncated,
        "filename": headers.filename, "size": digest.size, "sha256": digest.sha256,
    });
    let suffix = if headers.content_type_truncated {
        "…"
    } else {
        ""
    };
    let (summary, content) = match location {
        DownloadLocation::Retained(reference) => {
            let selector = json!({"id": reference.id});
            let content = format!(
                "HTTP {} returned {} bytes with Content-Type {}{suffix}. SHA-256 {}. Read the body with read_output({selector}). Use next_offset while has_more. Optional search filters text by a literal. Segments include raw_bytes when display text differs.",
                headers.status, digest.size, headers.content_type, digest.sha256,
            );
            facts["output_ref"] = json!(reference);
            facts["capture_complete"] = json!(true);
            (format!("downloaded {} bytes", digest.size), content)
        }
        DownloadLocation::Temporary(path) => {
            let path = path.to_string_lossy();
            let content = format!(
                "file_path: {path}\nstatus: {}\ncontent_type: {}{suffix}\nsize: {} bytes",
                headers.status, headers.content_type, digest.size,
            );
            facts["file_path"] = json!(path);
            (
                format!("downloaded {path} ({} bytes)", digest.size),
                content,
            )
        }
    };
    if headers.content_type_truncated {
        ToolResultEnvelope::ok_truncated(summary, content, facts)
    } else {
        ToolResultEnvelope::ok(summary, content, facts)
    }
}

async fn until_cancelled<T>(
    cancel: &Notify,
    operation: impl Future<Output = T>,
) -> Result<T, ToolResultEnvelope> {
    tokio::select! {
        biased;
        _ = cancel.notified() => Err(ToolResultEnvelope::cancelled("download cancelled")),
        result = operation => Ok(result),
    }
}

fn retained_write_error(error: std::io::Error) -> ToolResultEnvelope {
    ToolResultEnvelope::output_failure(
        "download output failed",
        error.to_string(),
        "fetch_url",
        false,
    )
}

fn temp_write_error(error: std::io::Error) -> ToolResultEnvelope {
    ToolResultEnvelope::error(
        "failed: write error",
        format!("failed to write download: {error}"),
    )
}

fn size_error(size: u64) -> ToolResultEnvelope {
    ToolResultEnvelope::error(
        "failed: file too large",
        format!("download exceeds 50 MiB limit ({size} bytes)"),
    )
}

fn quota_error() -> ToolResultEnvelope {
    ToolResultEnvelope::error(
        "failed: download exceeds output quota",
        "The response body does not fit the retained output quota.",
    )
}

fn extract_filename(url: &str, headers: &wreq::header::HeaderMap) -> String {
    if let Some(cd) = headers
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
    {
        if let Some(name) = cd.split("filename=").nth(1) {
            let name = name.trim_matches('"').trim();
            if !name.is_empty() {
                return sanitize_filename(name);
            }
        }
    }
    if let Some(path) = url.split('?').next() {
        if let Some(segment) = path.rsplit('/').next() {
            if !segment.is_empty() && segment.contains('.') {
                return sanitize_filename(segment);
            }
        }
    }
    let hash = format!("{:x}", Sha256::digest(url.as_bytes()));
    format!("fetch_{}", &hash[..12])
}

fn sanitize_filename(name: &str) -> String {
    let filtered: String = name
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(*c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
        })
        .take(255)
        .collect();
    if filtered.is_empty() || filtered == ".." || filtered == "." {
        String::from("download")
    } else {
        filtered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_url_rejects_non_http() {
        assert_eq!(
            validate_url("ftp://example.com").unwrap_err().status,
            "error"
        );
        assert_eq!(
            validate_url("file:///etc/passwd").unwrap_err().status,
            "error"
        );
    }

    #[test]
    fn validate_url_accepts_http_and_https() {
        assert!(validate_url("http://example.com").is_ok());
        assert!(validate_url("https://example.com/path").is_ok());
        assert!(validate_url("https://example.com?info=user@example.com:x").is_ok());
    }

    #[test]
    fn validate_url_rejects_too_long() {
        let long_url = format!("https://example.com/{}", "a".repeat(MAX_URL_BYTES));
        assert_eq!("error", validate_url(&long_url).unwrap_err().status);
    }

    #[test]
    fn validate_url_rejects_credentials() {
        for url in ["https://user:pass@example.com", "https://user@example.com"] {
            assert_eq!("error", validate_url(url).unwrap_err().status);
        }
    }

    #[test]
    fn extract_filename_from_content_disposition() {
        let mut headers = wreq::header::HeaderMap::new();
        headers.insert(
            "content-disposition",
            "attachment; filename=\"report.pdf\"".parse().unwrap(),
        );
        assert_eq!(extract_filename("https://x.com/", &headers), "report.pdf");
    }

    #[test]
    fn extract_filename_from_url_path() {
        let headers = wreq::header::HeaderMap::new();
        assert_eq!(
            extract_filename("https://example.com/files/data.json", &headers),
            "data.json"
        );
    }

    #[test]
    fn extract_filename_fallback_to_hash() {
        let headers = wreq::header::HeaderMap::new();
        let name = extract_filename("https://example.com/", &headers);
        assert!(name.starts_with("fetch_"));
    }

    #[test]
    fn sanitize_filename_dot_dot_returns_download() {
        assert_eq!(sanitize_filename(".."), "download");
    }

    #[test]
    fn sanitize_filename_dot_returns_download() {
        assert_eq!(sanitize_filename("."), "download");
    }

    #[test]
    fn sanitize_filename_normal() {
        assert_eq!(sanitize_filename("normal.txt"), "normal.txt");
    }

    #[test]
    fn sanitize_filename_strips_slashes() {
        assert_eq!(sanitize_filename("../etc/passwd"), "..etcpasswd");
    }

    #[test]
    fn sanitize_filename_strips_angle_brackets() {
        assert_eq!(sanitize_filename("file<1>.txt"), "file1.txt");
    }
}
