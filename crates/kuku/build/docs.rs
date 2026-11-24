use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::ops::Range;
use std::path::{Component, Path};

use flate2::{Compression, GzBuilder};
use pulldown_cmark::{Event, LinkType, Parser, Tag};

use crate::bundle_format::{
    FileEntry, HEADER, MAX_CONTENT_BYTES, MAX_FILE_BYTES, MAX_MANIFEST_BYTES, Manifest,
    fingerprint, valid_path,
};

pub(crate) fn build(root: &Path, revision: &str, out: &Path) -> io::Result<String> {
    let mut files = BTreeMap::new();
    let root = fs::canonicalize(root)?;
    collect(&root, &root, &mut files)?;
    let total = files.values().try_fold(0_usize, |total, bytes| {
        total
            .checked_add(bytes.len())
            .filter(|total| *total <= MAX_CONTENT_BYTES)
    });
    if total.is_none() || files.is_empty() {
        return Err(io::Error::other("invalid bundled documentation size"));
    }
    for (path, bytes) in &mut files {
        if path.ends_with(".md") {
            let text = std::str::from_utf8(bytes).map_err(io::Error::other)?;
            let text = text.replace("\r\n", "\n");
            *bytes = rewrite_links(path, &text, revision)?.into_bytes();
        }
    }
    if files.values().map(Vec::len).sum::<usize>() > MAX_CONTENT_BYTES
        || files.values().any(|bytes| bytes.len() > MAX_FILE_BYTES)
    {
        return Err(io::Error::other(
            "bundled documentation exceeds size limits",
        ));
    }
    let manifest = Manifest {
        source_revision: revision.to_owned(),
        files: files
            .iter()
            .map(|(path, bytes)| {
                (
                    path.clone(),
                    FileEntry {
                        bytes: bytes.len(),
                        sha256: fingerprint(bytes),
                    },
                )
            })
            .collect(),
    };
    let manifest = serde_json::to_vec(&manifest)?;
    if manifest.len() > MAX_MANIFEST_BYTES {
        return Err(io::Error::other(
            "bundled documentation manifest exceeds size limit",
        ));
    }
    let bundle = fs::File::create(out.join("kuku-docs.bundle"))?;
    let mut bundle = io::BufWriter::new(bundle);
    bundle.write_all(HEADER)?;
    let length = u32::try_from(manifest.len()).map_err(io::Error::other)?;
    bundle.write_all(&length.to_be_bytes())?;
    bundle.write_all(&manifest)?;
    let mut content = GzBuilder::new().write(bundle, Compression::best());
    for bytes in files.values() {
        content.write_all(bytes)?;
    }
    content.finish()?.flush()?;
    Ok(fingerprint(&manifest))
}

fn collect(root: &Path, dir: &Path, files: &mut BTreeMap<String, Vec<u8>>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect(root, &path, files)?;
            continue;
        }
        if file_type.is_symlink() {
            let target = fs::canonicalize(&path)?;
            if !target.starts_with(root) || !target.is_file() {
                return Err(io::Error::other(format!(
                    "invalid docs link at {}",
                    path.display()
                )));
            }
        } else if !file_type.is_file() {
            return Err(io::Error::other(format!(
                "unsupported docs entry at {}",
                path.display()
            )));
        }
        let relative = path.strip_prefix(root).expect("path is below docs root");
        let key = relative
            .iter()
            .map(|part| {
                part.to_str()
                    .ok_or_else(|| io::Error::other("non-UTF-8 docs path"))
            })
            .collect::<io::Result<Vec<_>>>()?
            .join("/");
        if !valid_path(&key) || fs::metadata(&path)?.len() > MAX_FILE_BYTES as u64 {
            return Err(io::Error::other(format!("invalid docs file {key}")));
        }
        let mut bytes = fs::read(&path)?;
        // Git on Windows can check out an existing symlink as its target text.
        if key == "CLAUDE.md" && bytes == b"AGENTS.md" {
            bytes = fs::read(root.join("AGENTS.md"))?;
        }
        files.insert(key, bytes);
    }
    Ok(())
}

fn rewrite_links(path: &str, text: &str, revision: &str) -> io::Result<String> {
    if revision.is_empty() {
        return Ok(text.to_owned());
    }
    let parser = Parser::new(text);
    let mut replacements = BTreeMap::<usize, (Range<usize>, String)>::new();
    let mut add = |span: Range<usize>, marker: &str, destination: &str| -> io::Result<()> {
        let Some(href) = repository_link(path, destination, revision)? else {
            return Ok(());
        };
        let start =
            destination_start(&text[span.clone()], marker, destination).ok_or_else(|| {
                io::Error::other(format!(
                    "cannot locate repository link {destination:?} in {path}"
                ))
            })? + span.start;
        replacements.insert(start, (start..start + destination.len(), href));
        Ok(())
    };
    for (_, definition) in parser.reference_definitions().iter() {
        add(definition.span.clone(), "]:", &definition.dest)?;
    }
    for (event, span) in parser.into_offset_iter() {
        match event {
            Event::Start(Tag::Link {
                link_type: LinkType::Inline,
                dest_url,
                ..
            })
            | Event::Start(Tag::Image {
                link_type: LinkType::Inline,
                dest_url,
                ..
            }) => {
                add(span, "](", &dest_url)?;
            }
            _ => {}
        }
    }
    let mut text = text.to_owned();
    for (_, (range, href)) in replacements.into_iter().rev() {
        text.replace_range(range, &href);
    }
    Ok(text)
}

fn destination_start(span: &str, marker: &str, destination: &str) -> Option<usize> {
    span.rmatch_indices(marker).find_map(|(index, marker)| {
        let after = &span[index + marker.len()..];
        let start = after.trim_start().trim_start_matches('<');
        let suffix = start.strip_prefix(destination)?;
        (suffix.is_empty() || suffix.starts_with([')', '>', ' ', '\n', '\t']))
            .then_some(span.len() - start.len())
    })
}

fn repository_link(path: &str, destination: &str, revision: &str) -> io::Result<Option<String>> {
    let pathname_end = destination.find(['#', '?']).unwrap_or(destination.len());
    let (pathname, suffix) = destination.split_at(pathname_end);
    if pathname.is_empty() || pathname.starts_with('/') || pathname.contains(':') {
        return Ok(None);
    }
    let mut components = vec!["docs"];
    if let Some((parent, _)) = path.rsplit_once('/') {
        components.extend(parent.split('/'));
    }
    for component in Path::new(pathname).components() {
        match component {
            Component::Normal(part) => {
                components.push(
                    part.to_str()
                        .ok_or_else(|| io::Error::other("non-UTF-8 docs link"))?,
                );
            }
            Component::ParentDir => {
                components.pop().ok_or_else(|| {
                    io::Error::other(format!(
                        "docs link escapes the repository in {path}: {destination}"
                    ))
                })?;
            }
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => return Ok(None),
        }
    }
    if components.first() == Some(&"docs") {
        return Ok(None);
    }
    let repo = env!("CARGO_PKG_REPOSITORY");
    let href = if components.is_empty() {
        format!("{repo}/tree/{revision}{suffix}")
    } else {
        format!("{repo}/blob/{revision}/{}{suffix}", components.join("/"))
    };
    Ok(Some(href))
}
