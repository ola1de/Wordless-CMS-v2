//! Base64url (no padding) encode/decode — the same alphabet v1 uses
//! for salts, hashes, and session tokens (`password.ts`, `auth/service.ts`).

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

pub fn encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    URL_SAFE_NO_PAD.decode(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_alphabet() {
        let bytes: Vec<u8> = (0..=255).collect();
        let encoded = encode(&bytes);
        assert!(!encoded.contains(['+', '/', '=']));
        assert_eq!(decode(&encoded).unwrap(), bytes);
        assert_eq!(encode(b""), "");
        assert_eq!(decode("").unwrap(), Vec::<u8>::new());
        assert!(decode("!!!").is_err());
    }
}
