//! Page 0 dual shadow header (`StorageHeader`) encoder, decoder, and slot selection
//! (`docs/file-format.md` §4.1).

use oryq_types::{ErrorCategory, ErrorEnvelope, FeatureBitmask, PageId, Result, TxId};
use xxhash_rust::xxh3::xxh3_64;

use crate::config::PageSize;

const MODULE_ID: &str = "engine";

pub const ORYQ_MAGIC: [u8; 8] = *b"ORYQDB\x00\x01";
pub const FORMAT_VERSION: u16 = 1;
pub const STORAGE_HEADER_SIZE: usize = 256;
pub const SLOT_A_OFFSET: usize = 0;
pub const SLOT_B_OFFSET: usize = 256;

/// Identifies which of the two Page 0 shadow header slots is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HeaderSlot {
    SlotA,
    SlotB,
}

impl HeaderSlot {
    #[inline]
    pub const fn offset(self) -> usize {
        match self {
            Self::SlotA => SLOT_A_OFFSET,
            Self::SlotB => SLOT_B_OFFSET,
        }
    }

    #[inline]
    pub const fn alternate(self) -> Self {
        match self {
            Self::SlotA => Self::SlotB,
            Self::SlotB => Self::SlotA,
        }
    }
}

/// 256-byte persistent database header stored redundantly in Page 0 Slot A and Slot B.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageHeader {
    pub format_version: u16,
    pub page_size: PageSize,
    pub header_generation: u64,
    pub required_read_features: FeatureBitmask,
    pub required_write_features: FeatureBitmask,
    pub database_uuid: [u8; 16],
    pub total_page_count: u64,
    pub catalog_root_page_id: PageId,
    pub freelist_trunk_page_id: u64,
    pub freelist_page_count: u64,
    pub checkpointed_tx_id: TxId,
    pub checkpointed_wal_seq: u64,
    pub reset_intent_seq: u64,
    pub wal_required: bool,
}

impl StorageHeader {
    /// Creates an initial v1 header for a newly initialized database file.
    ///
    /// A brand-new file has Page 0 (headers) and Page 1 (empty catalog root), so
    /// `total_page_count = 2` and `catalog_root_page_id = PageId(1)`.
    pub fn new_initial(page_size: PageSize, database_uuid: [u8; 16]) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            page_size,
            header_generation: 1,
            required_read_features: FeatureBitmask::EMPTY,
            required_write_features: FeatureBitmask::EMPTY,
            database_uuid,
            total_page_count: 2,
            catalog_root_page_id: PageId::new(1),
            freelist_trunk_page_id: 0,
            freelist_page_count: 0,
            checkpointed_tx_id: TxId::INITIAL,
            checkpointed_wal_seq: 0,
            reset_intent_seq: 0,
            wal_required: false,
        }
    }

    /// Encodes this header into an exact 256-byte little-endian slot with trailing `XXH3_64`.
    pub fn encode(&self, out: &mut [u8; STORAGE_HEADER_SIZE]) {
        out.fill(0);
        out[0..8].copy_from_slice(&ORYQ_MAGIC);
        out[8..10].copy_from_slice(&self.format_version.to_le_bytes());
        out[10..12].copy_from_slice(&self.page_size.log2().to_le_bytes());
        out[12..20].copy_from_slice(&self.header_generation.to_le_bytes());
        out[20..28].copy_from_slice(&self.required_read_features.raw().to_le_bytes());
        out[28..36].copy_from_slice(&self.required_write_features.raw().to_le_bytes());
        out[36..52].copy_from_slice(&self.database_uuid);
        out[52..60].copy_from_slice(&self.total_page_count.to_le_bytes());
        out[60..68].copy_from_slice(&self.catalog_root_page_id.raw().to_le_bytes());
        out[68..76].copy_from_slice(&self.freelist_trunk_page_id.to_le_bytes());
        out[76..84].copy_from_slice(&self.freelist_page_count.to_le_bytes());
        out[84..92].copy_from_slice(&self.checkpointed_tx_id.raw().to_le_bytes());
        out[92..100].copy_from_slice(&self.checkpointed_wal_seq.to_le_bytes());
        out[100..108].copy_from_slice(&self.reset_intent_seq.to_le_bytes());
        out[108] = u8::from(self.wal_required);
        // Bytes 109..248 remain 0x00.
        let checksum = xxh3_64(&out[0..248]);
        out[248..256].copy_from_slice(&checksum.to_le_bytes());
    }

    /// Decodes and validates a 256-byte shadow header slot.
    pub fn decode(buf: &[u8; STORAGE_HEADER_SIZE]) -> Result<Self> {
        if buf[0..8] != ORYQ_MAGIC {
            return Err(corrupt(
                "StorageHeader::decode",
                "invalid ORYQDB magic bytes",
            ));
        }
        let stored_checksum = u64::from_le_bytes(buf[248..256].try_into().unwrap());
        let computed = xxh3_64(&buf[0..248]);
        if stored_checksum != computed {
            return Err(corrupt(
                "StorageHeader::decode",
                format!(
                    "header checksum mismatch: stored=0x{stored_checksum:016x}, computed=0x{computed:016x}"
                ),
            ));
        }

        let format_version = u16::from_le_bytes([buf[8], buf[9]]);
        if format_version != FORMAT_VERSION {
            return Err(ErrorEnvelope::new(
                ErrorCategory::UnsupportedFeature,
                MODULE_ID,
                "StorageHeader::decode",
                format!("unsupported format_version {format_version}; expected {FORMAT_VERSION}"),
            ));
        }

        let page_size_log2 = u16::from_le_bytes([buf[10], buf[11]]);
        let page_size = PageSize::from_log2(page_size_log2)?;

        let wal_required = match buf[108] {
            0 => false,
            1 => true,
            other => {
                return Err(corrupt(
                    "StorageHeader::decode",
                    format!("invalid wal_required flag byte 0x{other:02x}"),
                ));
            }
        };

        if buf[109..248].iter().any(|&b| b != 0) {
            return Err(corrupt(
                "StorageHeader::decode",
                "non-zero bytes in reserved header region [109..248]",
            ));
        }

        let total_page_count = u64::from_le_bytes(buf[52..60].try_into().unwrap());
        if total_page_count < 2 {
            return Err(corrupt(
                "StorageHeader::decode",
                format!("total_page_count {total_page_count} must be >= 2"),
            ));
        }

        let raw_catalog_root = u64::from_le_bytes(buf[60..68].try_into().unwrap());
        if raw_catalog_root == 0 {
            return Err(corrupt(
                "StorageHeader::decode",
                "catalog_root_page_id cannot be Page 0",
            ));
        }
        let catalog_root_page_id = PageId::new(raw_catalog_root);
        if catalog_root_page_id.raw() >= total_page_count {
            return Err(corrupt(
                "StorageHeader::decode",
                format!(
                    "catalog_root_page_id {} exceeds total_page_count {total_page_count}",
                    catalog_root_page_id.raw()
                ),
            ));
        }

        let freelist_trunk_page_id = u64::from_le_bytes(buf[68..76].try_into().unwrap());
        if freelist_trunk_page_id >= total_page_count {
            return Err(corrupt(
                "StorageHeader::decode",
                format!(
                    "freelist_trunk_page_id {freelist_trunk_page_id} exceeds total_page_count {total_page_count}"
                ),
            ));
        }

        let mut database_uuid = [0u8; 16];
        database_uuid.copy_from_slice(&buf[36..52]);

        Ok(Self {
            format_version,
            page_size,
            header_generation: u64::from_le_bytes(buf[12..20].try_into().unwrap()),
            required_read_features: FeatureBitmask::from_raw(u64::from_le_bytes(
                buf[20..28].try_into().unwrap(),
            )),
            required_write_features: FeatureBitmask::from_raw(u64::from_le_bytes(
                buf[28..36].try_into().unwrap(),
            )),
            database_uuid,
            total_page_count,
            catalog_root_page_id,
            freelist_trunk_page_id,
            freelist_page_count: u64::from_le_bytes(buf[76..84].try_into().unwrap()),
            checkpointed_tx_id: TxId::new(u64::from_le_bytes(buf[84..92].try_into().unwrap())),
            checkpointed_wal_seq: u64::from_le_bytes(buf[92..100].try_into().unwrap()),
            reset_intent_seq: u64::from_le_bytes(buf[100..108].try_into().unwrap()),
            wal_required,
        })
    }
}

/// Validates both Slot A (`0..256`) and Slot B (`256..512`) and selects the valid slot
/// with the higher `header_generation`, without requiring the trailing reserved bytes.
pub fn select_header_slots(slots: &[u8]) -> Result<(HeaderSlot, StorageHeader)> {
    if slots.len() < SLOT_B_OFFSET + STORAGE_HEADER_SIZE {
        return Err(corrupt(
            "select_header_slots",
            format!("Page 0 header slice too small ({} bytes)", slots.len()),
        ));
    }

    let slot_a: &[u8; STORAGE_HEADER_SIZE] = slots
        [SLOT_A_OFFSET..SLOT_A_OFFSET + STORAGE_HEADER_SIZE]
        .try_into()
        .unwrap();
    let slot_b: &[u8; STORAGE_HEADER_SIZE] = slots
        [SLOT_B_OFFSET..SLOT_B_OFFSET + STORAGE_HEADER_SIZE]
        .try_into()
        .unwrap();

    let res_a = StorageHeader::decode(slot_a);
    let res_b = StorageHeader::decode(slot_b);

    match (res_a, res_b) {
        (Ok(a), Ok(b)) => {
            if b.header_generation > a.header_generation {
                Ok((HeaderSlot::SlotB, b))
            } else {
                Ok((HeaderSlot::SlotA, a))
            }
        }
        (Ok(a), Err(_)) => Ok((HeaderSlot::SlotA, a)),
        (Err(_), Ok(b)) => Ok((HeaderSlot::SlotB, b)),
        (Err(err_a), Err(_)) => Err(err_a),
    }
}

/// Validates both Slot A (`0..256`) and Slot B (`256..512`) of Page 0 and selects
/// the valid slot with the higher `header_generation`.
///
/// Also verifies that bytes `512..page_size` are zeroed.
pub fn select_active_header(page0: &[u8]) -> Result<(HeaderSlot, StorageHeader)> {
    let (chosen_slot, header) = select_header_slots(page0)?;

    let expected_len = header.page_size.bytes();
    if page0.len() < expected_len {
        return Err(corrupt(
            "select_active_header",
            format!(
                "Page 0 length {} is smaller than configured page_size {expected_len}",
                page0.len()
            ),
        ));
    }
    if page0[512..expected_len].iter().any(|&b| b != 0) {
        return Err(corrupt(
            "select_active_header",
            "Page 0 reserved area [512..page_size] contains non-zero bytes",
        ));
    }

    Ok((chosen_slot, header))
}

/// Encodes `header` into both Slot A and Slot B of a freshly zeroed Page 0 buffer.
pub fn write_page0_dual(header: &StorageHeader, page0: &mut [u8]) -> Result<()> {
    let expected_len = header.page_size.bytes();
    if page0.len() != expected_len {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "write_page0_dual",
            format!(
                "Page 0 buffer len {} != page_size {expected_len}",
                page0.len()
            ),
        ));
    }
    page0.fill(0);
    let mut encoded = [0u8; STORAGE_HEADER_SIZE];
    header.encode(&mut encoded);
    page0[SLOT_A_OFFSET..SLOT_A_OFFSET + STORAGE_HEADER_SIZE].copy_from_slice(&encoded);
    page0[SLOT_B_OFFSET..SLOT_B_OFFSET + STORAGE_HEADER_SIZE].copy_from_slice(&encoded);
    Ok(())
}

/// Encodes `header` into a single shadow slot (A or B) inside `page0`.
pub fn write_page0_slot(header: &StorageHeader, slot: HeaderSlot, page0: &mut [u8]) -> Result<()> {
    let expected_len = header.page_size.bytes();
    if page0.len() != expected_len {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "write_page0_slot",
            format!(
                "Page 0 buffer len {} != page_size {expected_len}",
                page0.len()
            ),
        ));
    }
    let mut encoded = [0u8; STORAGE_HEADER_SIZE];
    header.encode(&mut encoded);
    let off = slot.offset();
    page0[off..off + STORAGE_HEADER_SIZE].copy_from_slice(&encoded);
    Ok(())
}

fn corrupt(op: &'static str, msg: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCategory::Corruption, MODULE_ID, op, msg)
}
