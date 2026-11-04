//! The canonical ByT5 byte table (SPEC.md §4) — fixed, no vocab files.
//!
//! UTF-8 byte `b` maps to id `b + 3`; one trailing EOS (id 1) is
//! appended on encode; pad = 0, unk = 2. Ids are NOT raw bytes —
//! feeding `text.as_bytes()` directly produces silent garbage on real
//! models (the classic ByT5 interop bug; SPEC.md §4 warning).

/// Byte offset: id = byte + 3.
pub const BYTE_OFFSET: i64 = 3;
/// Special id 0: padding (skipped on decode).
pub const PAD_ID: i64 = 0;
/// Special id 1: end of sequence (stops decode; appended on encode).
pub const EOS_ID: i64 = 1;
/// Special id 2: unknown (skipped on decode).
pub const UNK_ID: i64 = 2;

/// `tokens.encode`: UTF-8 bytes b -> ids b+3, then one trailing EOS.
pub fn encode(text: &str) -> Vec<i64> {
    let mut ids: Vec<i64> = text
        .as_bytes()
        .iter()
        .map(|&b| i64::from(b) + BYTE_OFFSET)
        .collect();
    ids.push(EOS_ID);
    ids
}

/// `tokens.decode`: stop at EOS; skip pad/unk; `(id - 3) mod 256` per
/// byte; reassemble as UTF-8 (lossy, like the reference loader's
/// `errors="replace"`).
pub fn decode(token_ids: &[i64]) -> String {
    let mut bytes = Vec::with_capacity(token_ids.len());
    for &token in token_ids {
        if token == EOS_ID {
            break;
        }
        if token == PAD_ID || token == UNK_ID {
            continue;
        }
        bytes.push((token - BYTE_OFFSET).rem_euclid(256) as u8);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_appends_offset_and_eos() {
        // "a" -> [97 + 3, EOS]
        assert_eq!(encode("a"), vec![100, EOS_ID]);
        // empty string still carries the trailing EOS (the reference
        // loader short-circuits on that shape).
        assert_eq!(encode(""), vec![EOS_ID]);
    }

    #[test]
    fn encode_multibyte_is_per_byte() {
        // Khmer "ខ" = U+1781 = UTF-8 0xE1 0x9E 0x81
        assert_eq!(encode("ខ"), vec![0xE1 + 3, 0x9E + 3, 0x81 + 3, EOS_ID]);
    }

    #[test]
    fn decode_roundtrip_ascii_and_multibyte() {
        let text = "ភាសា en ک";
        assert_eq!(decode(&encode(text)), text);
    }

    #[test]
    fn decode_stops_at_eos_and_skips_pad_unk() {
        // pad 0 / unk 2 are dropped, EOS truncates.
        // (token 100 -> byte 97 'a', token 101 -> byte 98 'b')
        assert_eq!(decode(&[PAD_ID, 100, UNK_ID, 101, EOS_ID, 102]), "ab");
    }

    #[test]
    fn decode_ids_below_offset_mod_256() {
        // (id - 3) mod 256: id 324 wraps to byte 65 'A'.
        assert_eq!(decode(&[324, EOS_ID]), "A");
    }
}
