use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;

const MAGIC: &[u8; 8] = b"KUKUJNL1";
const SIZE_HEADER: usize = 36;
const BLOCK_BYTES: usize = 1024 * 1024;
const MAX_ENCODED_BLOCK: usize = BLOCK_BYTES + 64 * 1024;
const MAX_FRAMES: usize = 262_144;
pub(super) const MAX_CHECKPOINT: usize = 32 * 1024 * 1024;
const MIN_UNPACKED_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Layout {
    frames: Vec<Frame>,
    checkpoint_bytes: usize,
    header_bytes: u64,
    pub(super) prefix_bytes: u64,
    pub(super) prefix_head: u64,
    pub(super) prefix_turns: u64,
    tail_at: u64,
}

struct Frame {
    start: u64,
    offset: u64,
    encoded: usize,
    decoded: usize,
}

impl Layout {
    pub(super) fn read(mut file: &File) -> io::Result<Self> {
        file.seek(SeekFrom::Start(0))?;
        let mut magic = [0; MAGIC.len()];
        match file.read_exact(&mut magic) {
            Ok(()) if &magic == MAGIC => {}
            Ok(()) => return plain_layout(magic[0]),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                file.rewind()?;
                if file.read(&mut magic)? == 0 {
                    return Ok(Self::default());
                }
                return plain_layout(magic[0]);
            }
            Err(error) => return Err(error),
        }
        let mut header = [0; SIZE_HEADER];
        header[..MAGIC.len()].copy_from_slice(MAGIC);
        file.read_exact(&mut header[MAGIC.len()..])?;
        let count = u32_at(&header, 8) as usize;
        let checkpoint_bytes = u32_at(&header, 12) as usize;
        let prefix_head = u64_at(&header, 16);
        let prefix_turns = u64_at(&header, 24);
        if count == 0 || count > MAX_FRAMES || checkpoint_bytes > MAX_CHECKPOINT || prefix_head == 0
        {
            return Err(invalid("invalid directory limits"));
        }
        let mut table = vec![0; count * 8];
        file.read_exact(&mut table)?;
        if u32_at(&header, 32) != directory_crc(&header, &table) {
            return Err(invalid("bad directory checksum"));
        }
        let header_bytes = (SIZE_HEADER + table.len()) as u64;
        let mut offset = header_bytes + checkpoint_bytes as u64;
        let mut logical = 0;
        let mut frames = Vec::with_capacity(count);
        for row in table.as_chunks::<8>().0 {
            let decoded = u32_at(row, 0) as usize;
            let encoded = u32_at(row, 4) as usize;
            if decoded == 0 || decoded > BLOCK_BYTES || encoded == 0 || encoded > MAX_ENCODED_BLOCK
            {
                return Err(invalid("invalid frame length"));
            }
            frames.push(Frame {
                start: logical,
                offset,
                decoded,
                encoded,
            });
            logical += decoded as u64;
            offset += encoded as u64;
        }
        if offset > file.metadata()?.len() {
            return Err(invalid("truncated committed frames"));
        }
        Ok(Self {
            frames,
            checkpoint_bytes,
            header_bytes,
            prefix_bytes: logical,
            prefix_head,
            prefix_turns,
            tail_at: offset,
        })
    }

    pub(super) fn logical_len(&self, physical_len: u64) -> u64 {
        self.prefix_bytes + physical_len.saturating_sub(self.tail_at)
    }

    pub(super) fn tail_offset(&self, logical: u64) -> io::Result<u64> {
        logical
            .checked_sub(self.prefix_bytes)
            .map(|offset| self.tail_at + offset)
            .ok_or_else(|| invalid("write into a sealed frame"))
    }

    pub(super) fn worth_packing(&self, logical_len: u64) -> bool {
        let tail = logical_len.saturating_sub(self.prefix_bytes);
        tail >= MIN_UNPACKED_BYTES.max(self.tail_at)
    }

    pub(super) fn has_checkpoint(&self) -> bool {
        self.checkpoint_bytes != 0
    }

    pub(super) fn checkpoint(&self, mut file: &File) -> io::Result<Option<Vec<u8>>> {
        if !self.has_checkpoint() {
            return Ok(None);
        }
        file.seek(SeekFrom::Start(self.header_bytes))?;
        let mut bytes = Vec::new();
        GzDecoder::new(file.take(self.checkpoint_bytes as u64))
            .take((MAX_CHECKPOINT + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_CHECKPOINT {
            return Err(invalid("oversized recovery checkpoint"));
        }
        Ok(Some(bytes))
    }

    pub(super) fn reader<'a>(&'a self, file: &'a File, end: u64, position: u64) -> Reader<'a> {
        Reader {
            layout: self,
            file,
            end,
            position,
            physical: None,
            loaded: None,
            frame: Vec::new(),
        }
    }

    pub(super) fn pack(
        &self,
        mut source: &File,
        path: &Path,
        end: u64,
        head: u64,
        turns: u64,
        checkpoint: PackCheckpoint<'_>,
    ) -> io::Result<Option<Staged>> {
        let tail = end
            .checked_sub(self.prefix_bytes)
            .ok_or_else(|| invalid("invalid seal boundary"))?;
        let count = self.frames.len() + tail.div_ceil(BLOCK_BYTES as u64) as usize;
        if count == 0 || count > MAX_FRAMES || checkpoint.bytes.len() > MAX_CHECKPOINT {
            return Ok(None);
        }
        let mut table = Vec::with_capacity(count * 8);
        let staging_path = staging_path(path);
        let file = File::create_new(&staging_path)?;
        let staging = Staged(staging_path);
        let mut replacement = file;
        replacement.set_permissions(source.metadata()?.permissions())?;
        let data_at = SIZE_HEADER + count * 8;
        replacement.seek(SeekFrom::Start(data_at as u64))?;
        replacement.write_all(checkpoint.bytes)?;
        if let Some(first) = self.frames.first() {
            source.seek(SeekFrom::Start(first.offset))?;
            let length = self.tail_at - first.offset;
            if io::copy(&mut source.take(length), &mut replacement)? != length {
                return Err(invalid("short read copying sealed frames"));
            }
            for frame in &self.frames {
                write_entry(&mut table, frame.decoded, frame.encoded);
            }
        }
        source.seek(SeekFrom::Start(self.tail_at))?;
        let mut buffer = vec![0; BLOCK_BYTES];
        let mut encoded = Vec::new();
        let mut remaining = tail;
        while remaining != 0 {
            let bytes = remaining.min(BLOCK_BYTES as u64) as usize;
            source.read_exact(&mut buffer[..bytes])?;
            let mut encoder = GzEncoder::new(&mut encoded, Compression::fast());
            encoder.write_all(&buffer[..bytes])?;
            encoder.finish()?;
            write_entry(&mut table, bytes, encoded.len());
            replacement.write_all(&encoded)?;
            encoded.clear();
            remaining -= bytes as u64;
        }
        let physical_end = replacement.stream_position()?;
        if !checkpoint.replace_invalid && physical_end >= source.metadata()?.len() {
            return Ok(None);
        }
        let mut header = [0; SIZE_HEADER];
        header[..8].copy_from_slice(MAGIC);
        header[8..12].copy_from_slice(&(count as u32).to_le_bytes());
        header[12..16].copy_from_slice(&(checkpoint.bytes.len() as u32).to_le_bytes());
        header[16..24].copy_from_slice(&head.to_le_bytes());
        header[24..32].copy_from_slice(&turns.to_le_bytes());
        let crc = directory_crc(&header, &table);
        header[32..].copy_from_slice(&crc.to_le_bytes());
        replacement.rewind()?;
        replacement.write_all(&header)?;
        replacement.write_all(&table)?;
        replacement.sync_all()?;
        drop(replacement);
        Ok(Some(staging))
    }
}

pub(super) struct PackCheckpoint<'a> {
    pub(super) bytes: &'a [u8],
    pub(super) replace_invalid: bool,
}

pub(super) struct Staged(pub(super) PathBuf);

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn staging_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap().to_os_string();
    name.push(".pack.tmp");
    path.with_file_name(name)
}

pub(super) fn clear_staging(path: &Path) -> io::Result<()> {
    match fs::remove_file(staging_path(path)) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

pub(super) struct Reader<'a> {
    layout: &'a Layout,
    file: &'a File,
    end: u64,
    position: u64,
    physical: Option<u64>,
    loaded: Option<usize>,
    frame: Vec<u8>,
}

impl Read for Reader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let available = self.end.saturating_sub(self.position);
        let wanted = buffer
            .len()
            .min(usize::try_from(available).unwrap_or(usize::MAX));
        if wanted == 0 {
            return Ok(0);
        }
        let read = if self.position >= self.layout.prefix_bytes {
            let at = self.layout.tail_offset(self.position)?;
            if self.physical != Some(at) {
                self.file.seek(SeekFrom::Start(at))?;
            }
            let count = self.file.read(&mut buffer[..wanted])?;
            self.physical = Some(at + count as u64);
            count
        } else {
            let index = self
                .layout
                .frames
                .partition_point(|frame| frame.start <= self.position)
                - 1;
            let entry = &self.layout.frames[index];
            if self.loaded != Some(index) {
                self.loaded = None;
                self.physical = None;
                self.file.seek(SeekFrom::Start(entry.offset))?;
                self.frame.clear();
                GzDecoder::new(self.file.take(entry.encoded as u64))
                    .take((entry.decoded + 1) as u64)
                    .read_to_end(&mut self.frame)?;
                if self.frame.len() != entry.decoded {
                    return Err(invalid("frame length differs from its directory"));
                }
                self.loaded = Some(index);
            }
            let start = (self.position - entry.start) as usize;
            let count = wanted.min(entry.decoded - start);
            buffer[..count].copy_from_slice(&self.frame[start..start + count]);
            count
        };
        self.position += read as u64;
        Ok(read)
    }
}

impl Seek for Reader<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let next = match pos {
            SeekFrom::Start(at) => Some(at),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
            SeekFrom::End(delta) => self.end.checked_add_signed(delta),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid journal seek"))?;
        self.position = next;
        Ok(next)
    }
}

fn plain_layout(first: u8) -> io::Result<Layout> {
    if first == b'{' || first.is_ascii_whitespace() {
        Ok(Layout::default())
    } else {
        Err(invalid("unknown journal header"))
    }
}

fn write_entry(table: &mut Vec<u8>, decoded: usize, encoded: usize) {
    table.extend_from_slice(&(decoded as u32).to_le_bytes());
    table.extend_from_slice(&(encoded as u32).to_le_bytes());
}

fn directory_crc(header: &[u8], table: &[u8]) -> u32 {
    let mut crc = crc32fast::Hasher::new();
    crc.update(&header[..32]);
    crc.update(table);
    crc.finalize()
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("journal archive: {message}"),
    )
}
