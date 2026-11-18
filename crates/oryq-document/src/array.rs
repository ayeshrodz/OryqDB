//! Zero-copy view over a validated `OryqDoc` array payload.

use crate::codec::read_value_at_descriptor;
use crate::types::ValueView;

/// Zero-copy borrowed view over an `OryqDoc` array:
/// `[elem_count: u32 LE][byte_len: u32 LE][elem_descriptors: u32 LE * elem_count][elem_heap...]`.
#[derive(Debug, Clone, Copy)]
pub struct ArrayView<'a> {
    bytes: &'a [u8],
    elem_count: usize,
}

impl<'a> ArrayView<'a> {
    pub(crate) fn from_validated_bytes(bytes: &'a [u8]) -> Self {
        let elem_count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        Self { bytes, elem_count }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.elem_count
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.elem_count == 0
    }

    #[inline]
    pub fn as_bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// Random-access element lookup in `O(1)` time via the 32-bit element descriptor table.
    pub fn get(&self, index: usize) -> Option<ValueView<'a>> {
        if index >= self.elem_count {
            return None;
        }
        let desc_off = 8 + index * 4;
        let desc = u32::from_le_bytes([
            self.bytes[desc_off],
            self.bytes[desc_off + 1],
            self.bytes[desc_off + 2],
            self.bytes[desc_off + 3],
        ]);
        Some(read_value_at_descriptor(self.bytes, desc))
    }

    /// Iterates over all array elements in order without heap allocation.
    pub fn iter(&self) -> ArrayIter<'a> {
        ArrayIter {
            view: *self,
            index: 0,
        }
    }
}

/// Zero-allocation iterator over an [`ArrayView`].
#[derive(Debug, Clone)]
pub struct ArrayIter<'a> {
    view: ArrayView<'a>,
    index: usize,
}

impl<'a> Iterator for ArrayIter<'a> {
    type Item = ValueView<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.view.get(self.index)?;
        self.index += 1;
        Some(item)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.view.len().saturating_sub(self.index);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for ArrayIter<'_> {}
