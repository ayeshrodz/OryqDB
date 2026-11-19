//! State-normalized top-level physical delta patches, newest-first resolution,
//! and pure compaction (`docs/file-format.md` §4.2 & `docs/document-model.md` §4.4).

pub mod compact;
pub mod normalize;
pub mod resolve;

pub use compact::compact_document;
pub use normalize::{
    bloom_mask_for_top_level, decode_physical_patch, encode_physical_patch, PatchEntry, PatchOp,
    PhysicalPatch, NO_PREV_DELTA_SLOT,
};
pub use resolve::resolve_path_with_patches;
