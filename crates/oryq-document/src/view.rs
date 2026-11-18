//! Zero-copy `O(log k)` borrowed view over a validated `OryqDoc` buffer.

use oryq_types::Result;

use crate::array::ArrayView;
use crate::codec::{
    field_name_hash_prefix, read_entry_name, read_value_at_descriptor, validate_document,
    DIR_ENTRY_SIZE, DOC_HEADER_SIZE,
};
use crate::path::FieldPath;
use crate::types::{Decimal128, ObjectId, Uuid, ValueView};

/// Zero-copy borrowed view over validated `OryqDoc` v1 bytes.
///
/// Field lookups perform `O(log k)` binary search over the sorted 12-byte field directory
/// using the big-endian `name_hash_prefix` and UTF-8 tie-breaking.
#[derive(Debug, Clone, Copy)]
pub struct DocumentView<'a> {
    bytes: &'a [u8],
    field_count: usize,
}

impl<'a> DocumentView<'a> {
    /// Validates `bytes` as an `OryqDoc` v1 payload and returns a zero-copy view.
    pub fn from_bytes(bytes: &'a [u8]) -> Result<Self> {
        validate_document(bytes)?;
        Ok(Self::from_validated_bytes(bytes))
    }

    pub(crate) fn from_validated_bytes(bytes: &'a [u8]) -> Self {
        let field_count = u16::from_le_bytes([bytes[2], bytes[3]]) as usize;
        Self { bytes, field_count }
    }

    #[inline]
    pub fn as_bytes(&self) -> &'a [u8] {
        self.bytes
    }

    #[inline]
    pub fn field_count(&self) -> usize {
        self.field_count
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.field_count == 0
    }

    /// `O(log k)` binary search for a single top-level field name.
    pub fn get_field(&self, name: &str) -> Option<ValueView<'a>> {
        let target_hash = field_name_hash_prefix(name.as_bytes());
        let target_bytes = name.as_bytes();
        let mut lo = 0usize;
        let mut hi = self.field_count;

        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let entry_off = DOC_HEADER_SIZE + mid * DIR_ENTRY_SIZE;
            let entry_hash = u32::from_be_bytes([
                self.bytes[entry_off],
                self.bytes[entry_off + 1],
                self.bytes[entry_off + 2],
                self.bytes[entry_off + 3],
            ]);
            match entry_hash.cmp(&target_hash) {
                core::cmp::Ordering::Less => lo = mid + 1,
                core::cmp::Ordering::Greater => hi = mid,
                core::cmp::Ordering::Equal => {
                    let entry_name = read_entry_name(self.bytes, entry_off);
                    match entry_name.as_bytes().cmp(target_bytes) {
                        core::cmp::Ordering::Less => lo = mid + 1,
                        core::cmp::Ordering::Greater => hi = mid,
                        core::cmp::Ordering::Equal => {
                            let val_desc = u32::from_le_bytes([
                                self.bytes[entry_off + 8],
                                self.bytes[entry_off + 9],
                                self.bytes[entry_off + 10],
                                self.bytes[entry_off + 11],
                            ]);
                            return Some(read_value_at_descriptor(self.bytes, val_desc));
                        }
                    }
                }
            }
        }
        None
    }

    /// Resolves a validated dotted path (`O(d log k)` across depth `d`).
    pub fn get_path(&self, path: &FieldPath) -> Option<ValueView<'a>> {
        let mut segments = path.segments();
        let first = segments.next()?;
        let mut current = self.get_field(first)?;
        for seg in segments {
            match current {
                ValueView::Document(sub) => {
                    current = sub.get_field(seg)?;
                }
                _ => return None,
            }
        }
        Some(current)
    }

    /// Convenience path lookup accepting a raw dotted string; validates path syntax first.
    pub fn get_path_str(&self, raw_path: &str) -> Result<Option<ValueView<'a>>> {
        let path = FieldPath::parse(raw_path)?;
        Ok(self.get_path(&path))
    }

    pub fn get_str(&self, path: &FieldPath) -> Option<&'a str> {
        match self.get_path(path)? {
            ValueView::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn get_i64(&self, path: &FieldPath) -> Option<i64> {
        match self.get_path(path)? {
            ValueView::Int64(v) => Some(v),
            _ => None,
        }
    }

    pub fn get_f64(&self, path: &FieldPath) -> Option<f64> {
        match self.get_path(path)? {
            ValueView::Float64(v) => Some(v),
            _ => None,
        }
    }

    pub fn get_bool(&self, path: &FieldPath) -> Option<bool> {
        match self.get_path(path)? {
            ValueView::Bool(b) => Some(b),
            _ => None,
        }
    }

    pub fn get_datetime_ms(&self, path: &FieldPath) -> Option<i64> {
        match self.get_path(path)? {
            ValueView::DateTimeUtcMs(ms) => Some(ms),
            _ => None,
        }
    }

    pub fn get_object_id(&self, path: &FieldPath) -> Option<ObjectId> {
        match self.get_path(path)? {
            ValueView::ObjectId(id) => Some(id),
            _ => None,
        }
    }

    pub fn get_uuid(&self, path: &FieldPath) -> Option<Uuid> {
        match self.get_path(path)? {
            ValueView::Uuid(id) => Some(id),
            _ => None,
        }
    }

    pub fn get_decimal128(&self, path: &FieldPath) -> Option<Decimal128> {
        match self.get_path(path)? {
            ValueView::Decimal128(d) => Some(d),
            _ => None,
        }
    }

    pub fn get_binary(&self, path: &FieldPath) -> Option<(u8, &'a [u8])> {
        match self.get_path(path)? {
            ValueView::Binary { subtype, bytes } => Some((subtype, bytes)),
            _ => None,
        }
    }

    pub fn get_doc(&self, path: &FieldPath) -> Option<DocumentView<'a>> {
        match self.get_path(path)? {
            ValueView::Document(d) => Some(d),
            _ => None,
        }
    }

    pub fn get_array(&self, path: &FieldPath) -> Option<ArrayView<'a>> {
        match self.get_path(path)? {
            ValueView::Array(a) => Some(a),
            _ => None,
        }
    }

    /// Iterates over all `(field_name, ValueView)` pairs in directory order.
    pub fn iter(&self) -> DocumentIter<'a> {
        DocumentIter {
            view: *self,
            index: 0,
        }
    }
}

/// Zero-allocation iterator over top-level fields in a [`DocumentView`].
#[derive(Debug, Clone)]
pub struct DocumentIter<'a> {
    view: DocumentView<'a>,
    index: usize,
}

impl<'a> Iterator for DocumentIter<'a> {
    type Item = (&'a str, ValueView<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.view.field_count {
            return None;
        }
        let entry_off = DOC_HEADER_SIZE + self.index * DIR_ENTRY_SIZE;
        let name = read_entry_name(self.view.bytes, entry_off);
        let val_desc = u32::from_le_bytes([
            self.view.bytes[entry_off + 8],
            self.view.bytes[entry_off + 9],
            self.view.bytes[entry_off + 10],
            self.view.bytes[entry_off + 11],
        ]);
        let val = read_value_at_descriptor(self.view.bytes, val_desc);
        self.index += 1;
        Some((name, val))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.view.field_count.saturating_sub(self.index);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for DocumentIter<'_> {}
