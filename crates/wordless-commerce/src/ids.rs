//! Random identifiers: UUID v4 strings for order ids and cart tokens
//! (v1 uses `crypto.randomUUID()` for both). No `uuid` crate is in the
//! workspace — the 16 random bytes come from `wordless-kernel`
//! (`getrandom`, JS `crypto.getRandomValues` on wasm32) and the version
//! / variant bits are set by hand per RFC 4122 §4.4.

use wordless_kernel::error::{internal_error, WordlessError};

/// A fresh random UUID v4 string, e.g.
/// `550e8400-e29b-41d4-a716-446655440000`.
pub fn new_uuid_v4() -> Result<String, WordlessError> {
    let mut bytes = wordless_kernel::crypto::random_bytes(16)
        .map_err(|e| internal_error(&format!("Could not generate a random identifier: {e}")))?;
    // Set the version (4) and variant (10xx) bits.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = |b: &[u8]| -> String { b.iter().map(|x| format!("{x:02x}")).collect() };
    Ok(format!(
        "{}-{}-{}-{}-{}",
        hex(&bytes[0..4]),
        hex(&bytes[4..6]),
        hex(&bytes[6..8]),
        hex(&bytes[8..10]),
        hex(&bytes[10..16]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_v4_shape() {
        let id = new_uuid_v4().unwrap();
        assert_eq!(id.len(), 36);
        assert_eq!(id.chars().nth(8), Some('-'));
        assert_eq!(id.chars().nth(14), Some('4'));
        assert!(matches!(id.chars().nth(19), Some('8' | '9' | 'a' | 'b')));
        assert_ne!(new_uuid_v4().unwrap(), id);
    }
}
