//! The content address of an asset.

use crate::error::{Error, Result};
use std::fmt;

/// How many hex digits a BLAKE3 hash is written as.
const HEX_DIGITS: usize = 64;

/// The prefix that names the hash function, so a later format can add one.
const PREFIX: &str = "blake3:";

/// The content address of an asset: `blake3:` followed by the 64 lowercase
/// hex digits of the BLAKE3 hash of its bytes.
///
/// An asset is identified by what it is, never by where it was. A resolver is
/// free to find the bytes in a library directory, a cache, or a download, and
/// the address is what says it found the right ones.
///
/// Only lowercase is accepted, so one set of bytes has exactly one address
/// and two presets naming the same capture agree character for character.
///
/// ```
/// use valrig_core::preset::{asset_hash, Hash};
///
/// let address = asset_hash(b"the bytes of a capture");
/// assert!(address.as_str().starts_with("blake3:"));
/// assert_eq!(Hash::parse(address.as_str()).unwrap(), address);
/// assert!(Hash::parse("blake3:not-64-hex-digits").is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hash(String);

impl Hash {
    /// Read the `blake3:…` form.
    ///
    /// # Errors
    ///
    /// [`Error::BadHash`] if `s` is not the prefix followed by exactly 64
    /// lowercase hex digits.
    pub fn parse(s: &str) -> Result<Self> {
        let hex = s
            .strip_prefix(PREFIX)
            .ok_or_else(|| Error::BadHash(s.to_string()))?;
        let well_formed = hex.len() == HEX_DIGITS
            && hex
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
        if well_formed {
            Ok(Self(s.to_string()))
        } else {
            Err(Error::BadHash(s.to_string()))
        }
    }

    /// The whole address, prefix included.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The address of these bytes.
    pub(crate) fn of(bytes: &[u8]) -> Self {
        Self(format!("{PREFIX}{}", blake3::hash(bytes).to_hex()))
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_lowercase_blake3() {
        let h = Hash::of(b"hello");
        assert_eq!(
            h.as_str(),
            "blake3:ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f"
        );
        assert_eq!(h.to_string(), h.as_str());
        assert_eq!(Hash::parse(h.as_str()).unwrap(), h);
    }

    #[test]
    fn anything_else_is_refused() {
        for bad in [
            "",
            "blake3:",
            "blake3:abc",
            "sha256:00",
            "ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f",
            "blake3:EA8F163DB38682925E4491C5E58D4BB3506EF8C14EB78A86E908C5624A67200F",
            "blake3:zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
            "blake3:ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f0",
        ] {
            assert!(
                matches!(Hash::parse(bad), Err(Error::BadHash(_))),
                "{bad:?} should not parse"
            );
        }
    }

    #[test]
    fn the_address_of_no_bytes_is_still_an_address() {
        assert!(Hash::parse(Hash::of(b"").as_str()).is_ok());
    }
}
