//! Canonical cross-type value comparator with exact rational numeric ordering.
//!
//! Matches the total ordering defined in `docs/indexing.md` Section 4.1:
//! `Null < Bool(false) < Bool(true) < NaN < Number (Int64/Float64/Decimal128)
//!  < DateTimeUtcMs < String < Binary < ObjectId < Uuid < Array < Document`.

use core::cmp::Ordering;

use crate::types::{DecodedDecimal128, ValueView};

/// Canonical total ordering across all [`ValueView`] variants.
pub fn compare_values(lhs: &ValueView<'_>, rhs: &ValueView<'_>) -> Ordering {
    let lr = type_rank(lhs);
    let rr = type_rank(rhs);
    if lr != rr {
        return lr.cmp(&rr);
    }

    match (lhs, rhs) {
        (ValueView::Null, ValueView::Null) => Ordering::Equal,
        (ValueView::Bool(a), ValueView::Bool(b)) => a.cmp(b),
        (ValueView::Int64(_), ValueView::Int64(_))
        | (ValueView::Int64(_), ValueView::Float64(_))
        | (ValueView::Int64(_), ValueView::Decimal128(_))
        | (ValueView::Float64(_), ValueView::Int64(_))
        | (ValueView::Float64(_), ValueView::Float64(_))
        | (ValueView::Float64(_), ValueView::Decimal128(_))
        | (ValueView::Decimal128(_), ValueView::Int64(_))
        | (ValueView::Decimal128(_), ValueView::Float64(_))
        | (ValueView::Decimal128(_), ValueView::Decimal128(_)) => {
            compare_numbers(&to_exact_num(lhs), &to_exact_num(rhs))
        }
        (ValueView::DateTimeUtcMs(a), ValueView::DateTimeUtcMs(b)) => a.cmp(b),
        (ValueView::String(a), ValueView::String(b)) => a.as_bytes().cmp(b.as_bytes()),
        (
            ValueView::Binary {
                subtype: sa,
                bytes: ba,
            },
            ValueView::Binary {
                subtype: sb,
                bytes: bb,
            },
        ) => ba.cmp(bb).then_with(|| sa.cmp(sb)),
        (ValueView::ObjectId(a), ValueView::ObjectId(b)) => a.cmp(b),
        (ValueView::Uuid(a), ValueView::Uuid(b)) => a.cmp(b),
        (ValueView::Array(a), ValueView::Array(b)) => {
            let min_len = a.len().min(b.len());
            for i in 0..min_len {
                let ord = compare_values(&a.get(i).unwrap(), &b.get(i).unwrap());
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            a.len().cmp(&b.len())
        }
        (ValueView::Document(a), ValueView::Document(b)) => {
            // Canonical key-sorted UTF-8 field comparison.
            let mut a_fields: Vec<(&str, ValueView<'_>)> = a.iter().collect();
            let mut b_fields: Vec<(&str, ValueView<'_>)> = b.iter().collect();
            a_fields.sort_unstable_by(|x, y| x.0.as_bytes().cmp(y.0.as_bytes()));
            b_fields.sort_unstable_by(|x, y| x.0.as_bytes().cmp(y.0.as_bytes()));
            let min_len = a_fields.len().min(b_fields.len());
            for i in 0..min_len {
                let key_ord = a_fields[i].0.as_bytes().cmp(b_fields[i].0.as_bytes());
                if key_ord != Ordering::Equal {
                    return key_ord;
                }
                let val_ord = compare_values(&a_fields[i].1, &b_fields[i].1);
                if val_ord != Ordering::Equal {
                    return val_ord;
                }
            }
            a_fields.len().cmp(&b_fields.len())
        }
        _ => unreachable!("same type_rank implies matching variants"),
    }
}

fn type_rank(v: &ValueView<'_>) -> u8 {
    match v {
        ValueView::Null => 0x05,
        ValueView::Bool(false) => 0x10,
        ValueView::Bool(true) => 0x11,
        ValueView::Float64(f) if f.is_nan() => 0x1F,
        ValueView::Decimal128(d) if matches!(d.decode(), DecodedDecimal128::NaN) => 0x1F,
        ValueView::Int64(_) | ValueView::Float64(_) | ValueView::Decimal128(_) => 0x20,
        ValueView::DateTimeUtcMs(_) => 0x30,
        ValueView::String(_) => 0x40,
        ValueView::Binary { .. } => 0x50,
        ValueView::ObjectId(_) => 0x60,
        ValueView::Uuid(_) => 0x61,
        ValueView::Array(_) => 0x70,
        ValueView::Document(_) => 0x80,
    }
}

#[derive(Debug, Clone, Copy)]
enum ExactNum {
    NaN,
    NegInf,
    PosInf,
    Zero,
    /// Exact non-zero finite value: `(-1)^negative * coeff * 2^pow2 * 5^pow5`
    Finite {
        negative: bool,
        coeff: u128,
        pow2: i32,
        pow5: i32,
    },
}

fn to_exact_num(v: &ValueView<'_>) -> ExactNum {
    match *v {
        ValueView::Int64(n) => {
            if n == 0 {
                ExactNum::Zero
            } else {
                ExactNum::Finite {
                    negative: n < 0,
                    coeff: n.unsigned_abs() as u128,
                    pow2: 0,
                    pow5: 0,
                }
            }
        }
        ValueView::Float64(f) => {
            if f.is_nan() {
                return ExactNum::NaN;
            }
            if f.is_infinite() {
                return if f.is_sign_negative() {
                    ExactNum::NegInf
                } else {
                    ExactNum::PosInf
                };
            }
            if f == 0.0 {
                // Canonicalize -0.0 == +0.0
                return ExactNum::Zero;
            }
            let bits = f.to_bits();
            let negative = (bits >> 63) != 0;
            let raw_exp = ((bits >> 52) & 0x7FF) as i32;
            let raw_mant = bits & 0x000F_FFFF_FFFF_FFFF;
            let (coeff, pow2) = if raw_exp == 0 {
                // Subnormal: mant * 2^(-1074)
                (raw_mant as u128, -1074)
            } else {
                // Normal: (2^52 + mant) * 2^(raw_exp - 1023 - 52)
                (((1u64 << 52) | raw_mant) as u128, raw_exp - 1075)
            };
            ExactNum::Finite {
                negative,
                coeff,
                pow2,
                pow5: 0,
            }
        }
        ValueView::Decimal128(d) => match d.decode() {
            DecodedDecimal128::NaN => ExactNum::NaN,
            DecodedDecimal128::Infinity { negative: true } => ExactNum::NegInf,
            DecodedDecimal128::Infinity { negative: false } => ExactNum::PosInf,
            DecodedDecimal128::Finite {
                negative,
                coefficient,
                exponent,
            } => {
                if coefficient == 0 {
                    ExactNum::Zero
                } else {
                    ExactNum::Finite {
                        negative,
                        coeff: coefficient,
                        pow2: exponent,
                        pow5: exponent,
                    }
                }
            }
        },
        _ => unreachable!("to_exact_num called on non-numeric ValueView"),
    }
}

fn compare_numbers(a: &ExactNum, b: &ExactNum) -> Ordering {
    match (a, b) {
        (ExactNum::NaN, ExactNum::NaN) => Ordering::Equal,
        (ExactNum::NaN, _) => Ordering::Less,
        (_, ExactNum::NaN) => Ordering::Greater,

        (ExactNum::NegInf, ExactNum::NegInf) => Ordering::Equal,
        (ExactNum::NegInf, _) => Ordering::Less,
        (_, ExactNum::NegInf) => Ordering::Greater,

        (ExactNum::PosInf, ExactNum::PosInf) => Ordering::Equal,
        (ExactNum::PosInf, _) => Ordering::Greater,
        (_, ExactNum::PosInf) => Ordering::Less,

        (ExactNum::Zero, ExactNum::Zero) => Ordering::Equal,
        (ExactNum::Zero, ExactNum::Finite { negative, .. }) => {
            if *negative {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        }
        (ExactNum::Finite { negative, .. }, ExactNum::Zero) => {
            if *negative {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        }
        (
            ExactNum::Finite {
                negative: na,
                coeff: ca,
                pow2: p2a,
                pow5: p5a,
            },
            ExactNum::Finite {
                negative: nb,
                coeff: cb,
                pow2: p2b,
                pow5: p5b,
            },
        ) => {
            if na != nb {
                return if *na {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
            let mag = compare_positive_magnitudes(*ca, *p2a, *p5a, *cb, *p2b, *p5b);
            if *na {
                mag.reverse()
            } else {
                mag
            }
        }
    }
}

/// Exact comparison of `ca * 2^p2a * 5^p5a` vs `cb * 2^p2b * 5^p5b` for positive `ca, cb`.
fn compare_positive_magnitudes(
    ca: u128,
    p2a: i32,
    p5a: i32,
    cb: u128,
    p2b: i32,
    p5b: i32,
) -> Ordering {
    // Fast-path approximate log2 bound to avoid big-int work when exponents differ widely.
    // log2(5) ~= 2.321928094887362
    let approx_a = (127 - ca.leading_zeros() as i32) as f64
        + (p2a as f64)
        + (p5a as f64) * core::f64::consts::LOG2_10
        - (p5a as f64);
    let approx_b = (127 - cb.leading_zeros() as i32) as f64
        + (p2b as f64)
        + (p5b as f64) * core::f64::consts::LOG2_10
        - (p5b as f64);
    if (approx_a - approx_b).abs() > 4.0 {
        return if approx_a < approx_b {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }

    // Exact cross-multiplication: move negative relative powers to the opposite side.
    let mut lhs = BigUint::from_u128(ca);
    let mut rhs = BigUint::from_u128(cb);

    let d2 = p2a - p2b;
    if d2 > 0 {
        lhs.shl_bits(d2 as u32);
    } else if d2 < 0 {
        rhs.shl_bits((-d2) as u32);
    }

    let d5 = p5a - p5b;
    if d5 > 0 {
        lhs.mul_pow5(d5 as u32);
    } else if d5 < 0 {
        rhs.mul_pow5((-d5) as u32);
    }

    lhs.cmp(&rhs)
}

/// Minimal base-2^32 unsigned big integer for exact rational numeric comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BigUint {
    limbs: Vec<u32>,
}

impl BigUint {
    fn from_u128(mut v: u128) -> Self {
        let mut limbs = Vec::with_capacity(4);
        while v > 0 {
            limbs.push((v & 0xFFFF_FFFF) as u32);
            v >>= 32;
        }
        Self { limbs }
    }

    fn shl_bits(&mut self, bits: u32) {
        if self.limbs.is_empty() || bits == 0 {
            return;
        }
        let word_shift = (bits / 32) as usize;
        let bit_shift = bits % 32;
        if bit_shift != 0 {
            let mut carry = 0u64;
            for limb in &mut self.limbs {
                let val = ((*limb as u64) << bit_shift) | carry;
                *limb = (val & 0xFFFF_FFFF) as u32;
                carry = val >> 32;
            }
            if carry != 0 {
                self.limbs.push(carry as u32);
            }
        }
        if word_shift > 0 {
            let mut prefixed = vec![0u32; word_shift];
            prefixed.append(&mut self.limbs);
            self.limbs = prefixed;
        }
    }

    fn mul_small(&mut self, factor: u32) {
        if self.limbs.is_empty() {
            return;
        }
        let mut carry = 0u64;
        for limb in &mut self.limbs {
            let prod = (*limb as u64) * (factor as u64) + carry;
            *limb = (prod & 0xFFFF_FFFF) as u32;
            carry = prod >> 32;
        }
        if carry != 0 {
            self.limbs.push(carry as u32);
        }
    }

    fn mul_pow5(&mut self, mut exp: u32) {
        // 5^13 = 1_220_703_125 fits in u32.
        while exp >= 13 {
            self.mul_small(1_220_703_125);
            exp -= 13;
        }
        if exp > 0 {
            let mut factor = 1u32;
            for _ in 0..exp {
                factor *= 5;
            }
            self.mul_small(factor);
        }
    }
}

impl Ord for BigUint {
    fn cmp(&self, other: &Self) -> Ordering {
        self.limbs.len().cmp(&other.limbs.len()).then_with(|| {
            for (&a, &b) in self.limbs.iter().rev().zip(other.limbs.iter().rev()) {
                let ord = a.cmp(&b);
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            Ordering::Equal
        })
    }
}

impl PartialOrd for BigUint {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
