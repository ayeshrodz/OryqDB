//! RAII temporary spill file manager with byte quota enforcement for external merge sort
//! (`docs/modules/query.md` §5).

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use oryq_document::DocumentBuf;
use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

const MODULE_ID: &str = "query";
static SPILL_SEQ: AtomicU64 = AtomicU64::new(1);

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
        let seq = SPILL_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("oryqdb_spill_{}_{seq}", std::process::id()));
        fs::create_dir_all(&dir).map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "SpillManager::new",
                format!("failed to create spill dir: {err}"),
            )
            .with_source(err)
        })?;
        Ok(Self {
            dir,
            files: Vec::new(),
            bytes_written: 0,
            max_spill_bytes,
        })
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

    /// Reads all documents back from a previously written spill run file.
    pub fn read_run(path: &PathBuf) -> Result<Vec<DocumentBuf>> {
        let file = File::open(path).map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "SpillManager::read_run",
                format!("failed to open spill run: {err}"),
            )
            .with_source(err)
        })?;
        let mut reader = BufReader::new(file);
        let mut docs = Vec::new();
        let mut len_buf = [0u8; 4];
        loop {
            match reader.read_exact(&mut len_buf) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(err) => {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::Io,
                        MODULE_ID,
                        "SpillManager::read_run",
                        format!("failed reading spill length: {err}"),
                    )
                    .with_source(err));
                }
            }
            let len = u32::from_le_bytes(len_buf) as usize;
            let mut raw = vec![0u8; len];
            reader.read_exact(&mut raw).map_err(|err| {
                ErrorEnvelope::new(
                    ErrorCategory::Io,
                    MODULE_ID,
                    "SpillManager::read_run",
                    format!("failed reading spill payload: {err}"),
                )
                .with_source(err)
            })?;
            docs.push(DocumentBuf::from_bytes(raw)?);
        }
        Ok(docs)
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
