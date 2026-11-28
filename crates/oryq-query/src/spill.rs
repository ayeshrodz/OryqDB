//! RAII temporary spill file manager with byte quota enforcement for external merge sort
//! (`docs/modules/query.md` §5).

use std::collections::hash_map::RandomState;
use std::fs::{self, DirBuilder, File};
use std::hash::{BuildHasher, Hasher};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_document::DocumentBuf;
use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

const MODULE_ID: &str = "query";
/// Maximum valid serialized `OryqDoc` length in a spill run (`256 MiB - 1`).
const MAX_SPILL_DOC_BYTES: usize = (1 << 28) - 1;
static SPILL_SEQ: AtomicU64 = AtomicU64::new(1);

fn random_spill_token() -> u64 {
    #[cfg(unix)]
    {
        if let Ok(mut f) = File::open("/dev/urandom") {
            let mut buf = [0u8; 8];
            if f.read_exact(&mut buf).is_ok() {
                return u64::from_le_bytes(buf);
            }
        }
    }
    let mut h = RandomState::new().build_hasher();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    h.write_u128(nanos);
    h.write_u32(std::process::id());
    h.write_u64(SPILL_SEQ.fetch_add(1, Ordering::Relaxed));
    h.finish()
}

/// Manages temporary spill run files for one query execution and removes all of them on drop.
#[derive(Debug)]
pub struct SpillManager {
    dir: PathBuf,
    files: Vec<PathBuf>,
    bytes_written: usize,
    max_spill_bytes: usize,
}

impl SpillManager {
    pub fn new(max_spill_bytes: usize) -> Result<Self> {
        let mut last_err = None;
        for _ in 0..16 {
            let seq = SPILL_SEQ.fetch_add(1, Ordering::Relaxed);
            let token = random_spill_token();
            let dir = std::env::temp_dir().join(format!(
                "oryqdb_spill_{}_{seq}_{token:016x}",
                std::process::id()
            ));
            let mut builder = DirBuilder::new();
            builder.recursive(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&dir) {
                Ok(()) => {
                    return Ok(Self {
                        dir,
                        files: Vec::new(),
                        bytes_written: 0,
                        max_spill_bytes,
                    });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    last_err = Some(err);
                    continue;
                }
                Err(err) => {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::Io,
                        MODULE_ID,
                        "SpillManager::new",
                        format!("failed to create spill dir: {err}"),
                    )
                    .with_source(err));
                }
            }
        }
        Err(ErrorEnvelope::new(
            ErrorCategory::Io,
            MODULE_ID,
            "SpillManager::new",
            format!(
                "failed to create unique spill dir after 16 attempts: {:?}",
                last_err
            ),
        ))
    }

    pub fn dir_path(&self) -> &PathBuf {
        &self.dir
    }

    pub fn bytes_written(&self) -> usize {
        self.bytes_written
    }

    pub fn run_count(&self) -> usize {
        self.files.len()
    }

    /// Writes a sorted run of `DocumentBuf`s to a new temporary file, enforcing `max_spill_bytes`.
    pub fn write_run(&mut self, docs: &[DocumentBuf]) -> Result<PathBuf> {
        let run_bytes: usize = docs.iter().map(|d| 4 + d.len()).sum();
        if self.bytes_written.saturating_add(run_bytes) > self.max_spill_bytes {
            return Err(ErrorEnvelope::new(
                ErrorCategory::QuotaExceeded,
                MODULE_ID,
                "SpillManager::write_run",
                format!(
                    "spill quota exceeded: {} + {} > {} bytes",
                    self.bytes_written, run_bytes, self.max_spill_bytes
                ),
            ));
        }

        let path = self.dir.join(format!("run_{:04}.spill", self.files.len()));
        let file = File::create(&path).map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "SpillManager::write_run",
                format!("failed to create spill run: {err}"),
            )
            .with_source(err)
        })?;
        let mut writer = BufWriter::new(file);
        for doc in docs {
            let bytes = doc.as_bytes();
            writer
                .write_all(&(bytes.len() as u32).to_le_bytes())
                .and_then(|()| writer.write_all(bytes))
                .map_err(|err| {
                    ErrorEnvelope::new(
                        ErrorCategory::Io,
                        MODULE_ID,
                        "SpillManager::write_run",
                        format!("failed to write spill run: {err}"),
                    )
                    .with_source(err)
                })?;
        }
        writer.flush().map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "SpillManager::write_run",
                format!("failed to flush spill run: {err}"),
            )
            .with_source(err)
        })?;
        self.bytes_written += run_bytes;
        self.files.push(path.clone());
        Ok(path)
    }

    /// Opens a streaming reader over a previously written spill run file.
    pub fn open_run(path: &PathBuf) -> Result<SpillRunReader> {
        let file = File::open(path).map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "SpillManager::open_run",
                format!("failed to open spill run: {err}"),
            )
            .with_source(err)
        })?;
        let file_len = file
            .metadata()
            .map_err(|err| {
                ErrorEnvelope::new(
                    ErrorCategory::Io,
                    MODULE_ID,
                    "SpillManager::open_run",
                    format!("failed to stat spill run: {err}"),
                )
                .with_source(err)
            })?
            .len();
        Ok(SpillRunReader {
            reader: BufReader::new(file),
            remaining: file_len,
        })
    }

    /// Reads all documents back from a previously written spill run file.
    pub fn read_run(path: &PathBuf) -> Result<Vec<DocumentBuf>> {
        let mut reader = Self::open_run(path)?;
        let mut docs = Vec::new();
        while let Some(doc) = reader.next_doc()? {
            docs.push(doc);
        }
        Ok(docs)
    }
}

/// Streaming forward reader over a single length-prefixed spill run file.
#[derive(Debug)]
pub struct SpillRunReader {
    reader: BufReader<File>,
    remaining: u64,
}

impl SpillRunReader {
    /// Reads and decodes the next document from this spill run, or `Ok(None)` at EOF.
    pub fn next_doc(&mut self) -> Result<Option<DocumentBuf>> {
        if self.remaining == 0 {
            return Ok(None);
        }
        if self.remaining < 4 {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "SpillRunReader::next_doc",
                format!(
                    "truncated spill record length header ({} trailing bytes)",
                    self.remaining
                ),
            ));
        }
        let mut len_buf = [0u8; 4];
        self.reader.read_exact(&mut len_buf).map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "SpillRunReader::next_doc",
                format!("failed reading spill length: {err}"),
            )
            .with_source(err)
        })?;
        self.remaining -= 4;
        let len = u32::from_le_bytes(len_buf) as usize;
        if len > MAX_SPILL_DOC_BYTES || (len as u64) > self.remaining {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "SpillRunReader::next_doc",
                format!(
                    "corrupt spill record length {len} exceeds remaining file size {}",
                    self.remaining
                ),
            ));
        }
        self.remaining -= len as u64;
        let mut raw = vec![0u8; len];
        self.reader.read_exact(&mut raw).map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "SpillRunReader::next_doc",
                format!("failed reading spill payload: {err}"),
            )
            .with_source(err)
        })?;
        Ok(Some(DocumentBuf::from_bytes(raw)?))
    }
}

impl Drop for SpillManager {
    fn drop(&mut self) {
        for f in &self.files {
            let _ = fs::remove_file(f);
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}
