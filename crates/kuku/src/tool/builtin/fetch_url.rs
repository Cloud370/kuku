use std::path::Path;

use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio_stream::StreamExt;

use crate::tool::ToolResultEnvelope;

const MAX_DOWNLOAD_BYTES: u64 = 50 * 1024 * 1024;

pub(crate) async fn fetch_url(args: &Value, _workspace: &Path) -> ToolResultEnvelope {
    let Some(url) = args.get("url").and_then(Value::as_str) else {
        return ToolResultEnvelope::error("failed: missing url", "fetch_url requires url");
    };

    if let Err(e) = validate_url(url) {
        return e;
    }

    match download(url).await {
        Ok(meta) => ToolResultEnvelope::ok(
            format!("downloaded {} ({} bytes)", meta.file_path, meta.size),
            format!(
                "file_path: {}\nstatus: {}\ncontent_type: {}\nsize: {} bytes",
                meta.file_path, meta.status, meta.content_type, meta.size
            ),
            serde_json::json!({
                "kind": "fetch_url",
                "url": url,
                "file_path": meta.file_path,
                "status": meta.status,
                "content_type": meta.content_type,
                "size": meta.size,
            }),
        ),
        Err(e) => e,
    }
}

pub(super) fn validate_url(url: &str) -> Result<(), ToolResultEnvelope> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ToolResultEnvelope::error(
            "failed: invalid url protocol",
            "url must start with http:// or https://",
        ));
    }
    if url.len() > 8192 {
        return Err(ToolResultEnvelope::error(
            "failed: url too long",
            "url exceeds 8192 characters",
        ));
    }
    if let Some(after_scheme) = url.split("://").nth(1) {
        let authority = before_path(after_scheme);
        if authority.contains('@') && authority.contains(':') {
            return Err(ToolResultEnvelope::error(
                "failed: url contains credentials",
                "url must not contain username:password",
            ));
        }
    }
    Ok(())
}

fn before_path(s: &str) -> &str {
    s.split('/').next().unwrap_or(s)
}

struct DownloadMeta {
    file_path: String,
    status: u16,
    content_type: String,
    size: u64,
}

async fn download(url: &str) -> Result<DownloadMeta, ToolResultEnvelope> {
    let client = crate::provider::http_client::fetch_client();
    let response = client.get(url).send().await.map_err(|e| {
        ToolResultEnvelope::error("failed: fetch error", format!("failed to fetch url: {e}"))
    })?;

    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    let filename = extract_filename(url, response.headers());
    let tmp_dir = std::env::temp_dir().join("kuku-fetch");
    std::fs::create_dir_all(&tmp_dir).map_err(|e| {
        ToolResultEnvelope::error(
            "failed: cannot create temp dir",
            format!("failed to create temp directory: {e}"),
        )
    })?;

    let file_path = tmp_dir.join(&filename);

    if let Some(len) = response.content_length() {
        if len > MAX_DOWNLOAD_BYTES {
            return Err(ToolResultEnvelope::error(
                "failed: file too large",
                format!("download exceeds 50MB limit ({len} bytes, reported by server)"),
            ));
        }
    }

    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(&file_path).await.map_err(|e| {
        ToolResultEnvelope::error("failed: write error", format!("failed to create file: {e}"))
    })?;

    let mut total: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| {
            ToolResultEnvelope::error(
                "failed: download error",
                format!("failed to read chunk: {e}"),
            )
        })?;
        total += chunk.len() as u64;
        if total > MAX_DOWNLOAD_BYTES {
            drop(file);
            let _ = tokio::fs::remove_file(&file_path).await;
            return Err(ToolResultEnvelope::error(
                "failed: file too large",
                format!("download exceeds 50MB limit ({total} bytes)"),
            ));
        }
        file.write_all(&chunk).await.map_err(|e| {
            ToolResultEnvelope::error("failed: write error", format!("failed to write file: {e}"))
        })?;
    }

    Ok(DownloadMeta {
        file_path: file_path.to_string_lossy().into_owned(),
        status,
        content_type,
        size: total,
    })
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
    let hash = {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(url.as_bytes());
        format!("{:x}", digest)[..12].to_string()
    };
    format!("fetch_{hash}")
}

fn sanitize_filename(name: &str) -> String {
    let filtered: String = name
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(*c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
        })
        .take(255)
        .collect();
    if filtered == ".." || filtered == "." {
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
    }

    #[test]
    fn validate_url_rejects_too_long() {
        let long_url = format!("https://example.com/{}", "a".repeat(8193));
        assert_eq!(validate_url(&long_url).unwrap_err().status, "error");
    }

    #[test]
    fn validate_url_rejects_credentials() {
        assert_eq!(
            validate_url("https://user:pass@example.com")
                .unwrap_err()
                .status,
            "error"
        );
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
