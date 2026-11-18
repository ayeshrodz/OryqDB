//! Write-Ahead Log sidecar (`.oryq-wal`) header, frame header, and commit payload codecs
//! (`docs/file-format.md` §4.3).

use oryq_types::{ErrorCategory, ErrorEnvelope, Result, TxId};
use xxhash_rust::xxh3::{xxh3_64, Xxh3};

use crate::config::PageSize;
use crate::format::header::FORMAT_VERSION;

const MODULE_ID: &str = "engine";

pub const WAL_MAGIC: [u8; 8] = *b"ORYQWAL\x01";
pub const WAL_HEADER_SIZE: usize = 64;
pub const WAL_FRAME_HEADER_SIZE: usize = 48;
pub const WAL_COMMIT_PAYLOAD_SIZE: usize = 48;

/// 64-byte `.oryq-wal` file header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalHeader {
    pub format_version: u16,
    pub page_size: PageSize,
    pub checkpoint_seq: u64,
    pub database_uuid: [u8; 16],
    pub salt1: u64,
    pub salt2: u64,
    pub header_checksum: u64,
}

impl WalHeader {
    pub fn new(
        page_size: PageSize,
        checkpoint_seq: u64,
        database_uuid: [u8; 16],
        salt1: u64,
        salt2: u64,
    ) -> Self {
        let mut hdr = Self {
            format_version: FORMAT_VERSION,
            page_size,
            checkpoint_seq,
            database_uuid,
            salt1,
            salt2,
            header_checksum: 0,
        };
        let mut buf = [0u8; WAL_HEADER_SIZE];
        hdr.encode(&mut buf);
        hdr
    }

    pub fn encode(&mut self, out: &mut [u8; WAL_HEADER_SIZE]) {
        out.fill(0);
        out[0..8].copy_from_slice(&WAL_MAGIC);
        out[8..10].copy_from_slice(&self.format_version.to_le_bytes());
        out[10..12].copy_from_slice(&self.page_size.log2().to_le_bytes());
        out[12..20].copy_from_slice(&self.checkpoint_seq.to_le_bytes());
        out[20..36].copy_from_slice(&self.database_uuid);
        out[36..44].copy_from_slice(&self.salt1.to_le_bytes());
        out[44..52].copy_from_slice(&self.salt2.to_le_bytes());
        // bytes 52..56 reserved zero
        let checksum = xxh3_64(&out[0..56]);
        self.header_checksum = checksum;
        out[56..64].copy_from_slice(&checksum.to_le_bytes());
    }

    pub fn decode(buf: &[u8; WAL_HEADER_SIZE]) -> Result<Self> {
        if buf[0..8] != WAL_MAGIC {
            return Err(corrupt("WalHeader::decode", "invalid ORYQWAL magic bytes"));
        }
        let stored_checksum = u64::from_le_bytes(buf[56..64].try_into().unwrap());
        let computed = xxh3_64(&buf[0..56]);
        if stored_checksum != computed {
            return Err(corrupt(
                "WalHeader::decode",
                format!(
                    "WAL header checksum mismatch: stored=0x{stored_checksum:016x}, computed=0x{computed:016x}"
                ),
            ));
        }
        let format_version = u16::from_le_bytes([buf[8], buf[9]]);
        if format_version != FORMAT_VERSION {
            return Err(ErrorEnvelope::new(
                ErrorCategory::UnsupportedFeature,
                MODULE_ID,
                "WalHeader::decode",
                format!("unsupported WAL format_version {format_version}"),
            ));
        }
        let page_size = PageSize::from_log2(u16::from_le_bytes([buf[10], buf[11]]))?;
        if buf[52..56] != [0, 0, 0, 0] {
            return Err(corrupt(
                "WalHeader::decode",
                "non-zero reserved bytes [52..56] in WAL header",
            ));
        }
        let mut database_uuid = [0u8; 16];
        database_uuid.copy_from_slice(&buf[20..36]);

        Ok(Self {
            format_version,
            page_size,
            checkpoint_seq: u64::from_le_bytes(buf[12..20].try_into().unwrap()),
            database_uuid,
            salt1: u64::from_le_bytes(buf[36..44].try_into().unwrap()),
            salt2: u64::from_le_bytes(buf[44..52].try_into().unwrap()),
            header_checksum: stored_checksum,
        })
    }
}

/// Discriminator byte for WAL frame payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum WalFrameKind {
    FullPage = 0x01,
    SlotDelta = 0x02,
    Commit = 0x03,
}

impl WalFrameKind {
    pub fn from_u8(raw: u8) -> Result<Self> {
        match raw {
            0x01 => Ok(Self::FullPage),
            0x02 => Ok(Self::SlotDelta),
            0x03 => Ok(Self::Commit),
            other => Err(corrupt(
                "WalFrameKind::from_u8",
                format!("unknown WAL frame_kind 0x{other:02x}"),
            )),
        }
    }
}

/// 48-byte WAL frame header with chained cumulative `XXH3_64` checksum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalFrameHeader {
    pub frame_seq: u64,
    pub tx_id: TxId,
    pub logical_page_id: u64,
    pub frame_kind: WalFrameKind,
    pub payload_byte_len: u32,
    pub salt1: u64,
    pub cumulative_checksum: u64,
}

impl WalFrameHeader {
    pub fn compute_cumulative_checksum(
        header_prefix_40: &[u8; 40],
        payload: &[u8],
        prev_checksum: u64,
    ) -> u64 {
        let mut hasher = Xxh3::with_seed(prev_checksum);
        hasher.update(header_prefix_40);
        hasher.update(payload);
        hasher.digest()
    }

    pub fn encode(
        &mut self,
        payload: &[u8],
        prev_checksum: u64,
        out: &mut [u8; WAL_FRAME_HEADER_SIZE],
    ) -> Result<()> {
        if payload.len() != self.payload_byte_len as usize {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "WalFrameHeader::encode",
                format!(
                    "payload length {} does not match header payload_byte_len {}",
                    payload.len(),
                    self.payload_byte_len
                ),
            ));
        }
        out.fill(0);
        out[0..8].copy_from_slice(&self.frame_seq.to_le_bytes());
        out[8..16].copy_from_slice(&self.tx_id.raw().to_le_bytes());
        out[16..24].copy_from_slice(&self.logical_page_id.to_le_bytes());
        out[24] = self.frame_kind as u8;
        // out[25..28] = 0 (frame_flags)
        out[28..32].copy_from_slice(&self.payload_byte_len.to_le_bytes());
        out[32..40].copy_from_slice(&self.salt1.to_le_bytes());

        let prefix: &[u8; 40] = out[0..40].try_into().unwrap();
        let checksum = Self::compute_cumulative_checksum(prefix, payload, prev_checksum);
        self.cumulative_checksum = checksum;
        out[40..48].copy_from_slice(&checksum.to_le_bytes());
        Ok(())
    }

    pub fn decode_and_verify(
        header_buf: &[u8; WAL_FRAME_HEADER_SIZE],
        payload: &[u8],
        prev_checksum: u64,
    ) -> Result<Self> {
        if header_buf[25..28] != [0, 0, 0] {
            return Err(corrupt(
                "WalFrameHeader::decode_and_verify",
                "non-zero reserved frame_flags in WAL frame header",
            ));
        }
        let payload_byte_len = u32::from_le_bytes(header_buf[28..32].try_into().unwrap());
        if payload.len() != payload_byte_len as usize {
            return Err(corrupt(
                "WalFrameHeader::decode_and_verify",
                format!(
                    "WAL payload slice len {} != declared payload_byte_len {payload_byte_len}",
                    payload.len()
                ),
            ));
        }
        let stored_checksum = u64::from_le_bytes(header_buf[40..48].try_into().unwrap());
        let prefix: &[u8; 40] = header_buf[0..40].try_into().unwrap();
        let computed = Self::compute_cumulative_checksum(prefix, payload, prev_checksum);
        if stored_checksum != computed {
            return Err(corrupt(
                "WalFrameHeader::decode_and_verify",
                format!(
                    "WAL frame cumulative checksum mismatch: stored=0x{stored_checksum:016x}, computed=0x{computed:016x}"
                ),
            ));
        }

        let frame_kind = WalFrameKind::from_u8(header_buf[24])?;
        let logical_page_id = u64::from_le_bytes(header_buf[16..24].try_into().unwrap());
        if frame_kind == WalFrameKind::Commit && logical_page_id != 0 {
            return Err(corrupt(
                "WalFrameHeader::decode_and_verify",
                "CommitFrame must have logical_page_id == 0",
            ));
        }

        Ok(Self {
            frame_seq: u64::from_le_bytes(header_buf[0..8].try_into().unwrap()),
            tx_id: TxId::new(u64::from_le_bytes(header_buf[8..16].try_into().unwrap())),
            logical_page_id,
            frame_kind,
            payload_byte_len,
            salt1: u64::from_le_bytes(header_buf[32..40].try_into().unwrap()),
            cumulative_checksum: stored_checksum,
        })
    }
}

/// Fixed 48-byte payload of a `CommitFrame` (`WalFrameKind::Commit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitPayload {
    pub first_frame_seq: u64,
    pub mutation_frame_count: u64,
    pub db_total_pages: u64,
    pub catalog_root: u64,
    pub freelist_root: u64,
    pub freelist_page_count: u64,
}

impl CommitPayload {
    pub fn encode(&self, out: &mut [u8; WAL_COMMIT_PAYLOAD_SIZE]) {
        out[0..8].copy_from_slice(&self.first_frame_seq.to_le_bytes());
        out[8..16].copy_from_slice(&self.mutation_frame_count.to_le_bytes());
        out[16..24].copy_from_slice(&self.db_total_pages.to_le_bytes());
        out[24..32].copy_from_slice(&self.catalog_root.to_le_bytes());
        out[32..40].copy_from_slice(&self.freelist_root.to_le_bytes());
        out[40..48].copy_from_slice(&self.freelist_page_count.to_le_bytes());
    }

    pub fn decode(buf: &[u8; WAL_COMMIT_PAYLOAD_SIZE]) -> Self {
        Self {
            first_frame_seq: u64::from_le_bytes(buf[0..8].try_into().unwrap()),
            mutation_frame_count: u64::from_le_bytes(buf[8..16].try_into().unwrap()),
            db_total_pages: u64::from_le_bytes(buf[16..24].try_into().unwrap()),
            catalog_root: u64::from_le_bytes(buf[24..32].try_into().unwrap()),
            freelist_root: u64::from_le_bytes(buf[32..40].try_into().unwrap()),
            freelist_page_count: u64::from_le_bytes(buf[40..48].try_into().unwrap()),
        }
    }
}

fn corrupt(op: &'static str, msg: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCategory::Corruption, MODULE_ID, op, msg)
}
