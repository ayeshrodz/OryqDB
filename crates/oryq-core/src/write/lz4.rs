//! Pure-safe, zero-dependency LZ4 raw block compressor and bounds-checked decompressor
//! (`#![forbid(unsafe_code)]`) for `RECORD_KIND_COMPRESSED` (`0x03`) document storage.

use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

const MODULE_ID: &str = "core";
const MIN_MATCH: usize = 4;
const HASH_BITS: u32 = 12;
const HASH_SIZE: usize = 1 << HASH_BITS;
const MAX_DISTANCE: usize = 65_535;
const LAST_LITERALS: usize = 5;
const MF_LIMIT: usize = 12;

#[inline]
fn hash4(input: &[u8], pos: usize) -> usize {
    let v = u32::from_le_bytes([input[pos], input[pos + 1], input[pos + 2], input[pos + 3]]);
    ((v.wrapping_mul(2_654_435_761)) >> (32 - HASH_BITS)) as usize
}

#[inline]
fn write_ext_len(out: &mut Vec<u8>, mut len: usize) {
    while len >= 255 {
        out.push(255);
        len -= 255;
    }
    out.push(len as u8);
}

/// Compresses `input` into the standard LZ4 raw block format.
pub fn compress_block(input: &[u8]) -> Vec<u8> {
    let len = input.len();
    let mut out = Vec::with_capacity(len.saturating_add(len / 255 + 16));
    if len < MF_LIMIT {
        emit_last_literals(&mut out, input);
        return out;
    }

    let mut table = [0u32; HASH_SIZE];
    let mut ip: usize = 0;
    let mut anchor: usize = 0;
    let mflimit = len - MF_LIMIT;
    let matchlimit = len - LAST_LITERALS;

    // Seed hash at position 0, then start scanning at position 1.
    table[hash4(input, 0)] = 0;
    ip += 1;

    while ip <= mflimit {
        let h = hash4(input, ip);
        let candidate = table[h] as usize;
        table[h] = ip as u32;

        let offset = ip.saturating_sub(candidate);
        if offset == 0
            || offset > MAX_DISTANCE
            || input[candidate..candidate + 4] != input[ip..ip + 4]
        {
            ip += 1;
            continue;
        }

        // Extend match forward up to `matchlimit`.
        let mut mlen = MIN_MATCH;
        while ip + mlen < matchlimit && input[candidate + mlen] == input[ip + mlen] {
            mlen += 1;
        }

        // Emit token, literals, offset, and match length.
        let lit_len = ip - anchor;
        let match_code = mlen - MIN_MATCH;
        let token = ((lit_len.min(15) as u8) << 4) | (match_code.min(15) as u8);
        out.push(token);

        if lit_len >= 15 {
            write_ext_len(&mut out, lit_len - 15);
        }
        out.extend_from_slice(&input[anchor..ip]);
        out.extend_from_slice(&(offset as u16).to_le_bytes());

        if match_code >= 15 {
            write_ext_len(&mut out, match_code - 15);
        }

        ip += mlen;
        anchor = ip;
        if ip <= mflimit {
            table[hash4(input, ip - 2)] = (ip - 2) as u32;
        }
    }

    emit_last_literals(&mut out, &input[anchor..]);
    out
}

fn emit_last_literals(out: &mut Vec<u8>, lits: &[u8]) {
    if lits.is_empty() {
        return;
    }
    let lit_len = lits.len();
    let token = (lit_len.min(15) as u8) << 4;
    out.push(token);
    if lit_len >= 15 {
        write_ext_len(out, lit_len - 15);
    }
    out.extend_from_slice(lits);
}

/// Decompresses an LZ4 raw block into exactly `expected_len` bytes, rejecting any truncated,
/// out-of-bounds, or oversized payload with `ErrorCategory::Corruption`.
pub fn decompress_block(input: &[u8], expected_len: usize, max_len: usize) -> Result<Vec<u8>> {
    if expected_len > max_len {
        return Err(corrupt(format!(
            "compressed record uncompressed length {expected_len} exceeds max {max_len}"
        )));
    }
    let mut out = Vec::with_capacity(expected_len);
    let mut pos = 0usize;

    while pos < input.len() {
        let token = input[pos];
        pos += 1;

        // 1. Literals
        let mut lit_len = usize::from(token >> 4);
        if lit_len == 15 {
            loop {
                let Some(&b) = input.get(pos) else {
                    return Err(corrupt("truncated LZ4 literal length"));
                };
                pos += 1;
                lit_len = lit_len
                    .checked_add(usize::from(b))
                    .ok_or_else(|| corrupt("LZ4 literal length overflow"))?;
                if b != 255 {
                    break;
                }
            }
        }
        let lit_end = pos
            .checked_add(lit_len)
            .ok_or_else(|| corrupt("LZ4 literal end overflow"))?;
        if lit_end > input.len() {
            return Err(corrupt("truncated LZ4 literal payload"));
        }
        if out.len().saturating_add(lit_len) > expected_len {
            return Err(corrupt(
                "LZ4 literal output exceeds declared uncompressed length",
            ));
        }
        out.extend_from_slice(&input[pos..lit_end]);
        pos = lit_end;

        // Standard LZ4 block ends immediately after the final literal sequence.
        if pos == input.len() {
            break;
        }

        // 2. Match offset (2-byte LE)
        if pos + 2 > input.len() {
            return Err(corrupt("truncated LZ4 match offset"));
        }
        let offset = usize::from(u16::from_le_bytes([input[pos], input[pos + 1]]));
        pos += 2;
        if offset == 0 || offset > out.len() {
            return Err(corrupt(format!(
                "invalid LZ4 match offset {offset} for output len {}",
                out.len()
            )));
        }

        // 3. Match length
        let mut match_len = usize::from(token & 0x0F) + MIN_MATCH;
        if (token & 0x0F) == 15 {
            loop {
                let Some(&b) = input.get(pos) else {
                    return Err(corrupt("truncated LZ4 match length"));
                };
                pos += 1;
                match_len = match_len
                    .checked_add(usize::from(b))
                    .ok_or_else(|| corrupt("LZ4 match length overflow"))?;
                if b != 255 {
                    break;
                }
            }
        }
        if out.len().saturating_add(match_len) > expected_len {
            return Err(corrupt(
                "LZ4 match output exceeds declared uncompressed length",
            ));
        }

        // Copy match bytes (supports overlapping RLE runs where `offset < match_len`).
        if offset >= match_len {
            let start = out.len() - offset;
            out.extend_from_within(start..start + match_len);
        } else {
            for _ in 0..match_len {
                let b = out[out.len() - offset];
                out.push(b);
            }
        }
    }

    if out.len() != expected_len {
        return Err(corrupt(format!(
            "LZ4 decompressed length {} does not match declared length {expected_len}",
            out.len()
        )));
    }
    Ok(out)
}

fn corrupt(msg: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope::new(
        ErrorCategory::Corruption,
        MODULE_ID,
        "lz4::decompress_block",
        msg,
    )
}
