//! Move-only bounded secret storage. Presentation may render a mask, never the value.
use crate::Error;
use zeroize::{Zeroize, Zeroizing};

pub const MAX_SECRET_BYTES: usize = 16 * 1024;

/// An editable UTF-8 secret. No Clone, Display or Serialize implementation.
/// The allocation is fixed while editing so growth never frees unwiped contents.
pub struct SecretString {
    bytes: Zeroizing<Vec<u8>>,
}

impl Default for SecretString {
    fn default() -> Self {
        Self::empty()
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretString(<redacted>)")
    }
}

impl SecretString {
    pub fn empty() -> Self {
        Self {
            bytes: Zeroizing::new(Vec::with_capacity(MAX_SECRET_BYTES)),
        }
    }

    /// Takes ownership and clears the supplied allocation, including on refusal.
    pub fn new(value: String) -> Result<Self, Error> {
        let value = Zeroizing::new(value);
        let mut secret = Self::empty();
        secret.push_str(&value)?;
        Ok(secret)
    }

    /// Explicit access for protected protocol/input code. Never format this value.
    pub fn expose_secret(&self) -> &str {
        // All writes accept UTF-8 and edits validate character boundaries.
        std::str::from_utf8(&self.bytes).expect("secret UTF-8 invariant")
    }
    pub fn len(&self) -> usize {
        self.bytes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
    pub fn char_count(&self) -> usize {
        self.expose_secret().chars().count()
    }
    pub fn clear(&mut self) {
        self.bytes.zeroize();
    }

    pub fn push_str(&mut self, text: &str) -> Result<(), Error> {
        self.insert_str(self.len(), text)
    }
    pub fn push(&mut self, ch: char) -> Result<(), Error> {
        let mut encoded = Zeroizing::new([0u8; 4]);
        self.push_str(ch.encode_utf8(&mut *encoded))
    }
    pub fn insert_str(&mut self, at: usize, text: &str) -> Result<(), Error> {
        if !self.expose_secret().is_char_boundary(at) {
            return Err(Error::usage("The secret editor position is invalid."));
        }
        if text.contains('\0') || self.len().saturating_add(text.len()) > MAX_SECRET_BYTES {
            return Err(Error::usage(
                "The protected input is too long or contains a zero byte.",
            ));
        }
        let old = self.len();
        self.bytes.resize(old + text.len(), 0);
        self.bytes.copy_within(at..old, at + text.len());
        self.bytes[at..at + text.len()].copy_from_slice(text.as_bytes());
        Ok(())
    }
    /// Removes the character at a byte boundary and wipes the vacated tail.
    pub fn remove(&mut self, at: usize) -> Result<(), Error> {
        if !self.expose_secret().is_char_boundary(at) || at >= self.len() {
            return Err(Error::usage("The secret editor position is invalid."));
        }
        let width = self.expose_secret()[at..]
            .chars()
            .next()
            .unwrap()
            .len_utf8();
        let old = self.len();
        self.bytes.copy_within(at + width..old, at);
        self.bytes[old - width..].zeroize();
        self.bytes.truncate(old - width);
        Ok(())
    }
    pub fn pop(&mut self) -> bool {
        let Some((at, _)) = self.expose_secret().char_indices().next_back() else {
            return false;
        };
        self.remove(at).is_ok()
    }
}

/// Only private wire structs can serialize a borrowed secret.
pub(crate) struct SecretRef<'a>(pub &'a str);
impl serde::Serialize for SecretRef<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.0)
    }
}

/// App-owned protocol storage; growth wipes the old allocation before freeing it.
pub(crate) struct PrivateBuffer {
    bytes: Zeroizing<Vec<u8>>,
    limit: usize,
}
impl PrivateBuffer {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            bytes: Zeroizing::new(Vec::with_capacity(limit.min(4096))),
            limit,
        }
    }
    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}
impl std::io::Write for PrivateBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let required = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| std::io::Error::other("protected payload limit exceeded"))?;
        if required > self.bytes.capacity() {
            let capacity = required
                .max(self.bytes.capacity().saturating_mul(2))
                .min(self.limit);
            let mut replacement = Zeroizing::new(Vec::with_capacity(capacity));
            replacement.extend_from_slice(&self.bytes);
            self.bytes = replacement; // old allocation is wiped on drop
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Large engine artifacts/keys are never debug-formatted or cloned. This private
/// deserializer moves its allocation straight into zeroizing ownership.
pub(crate) struct ProtectedText(pub zeroize::Zeroizing<String>);
impl<'de> serde::Deserialize<'de> for ProtectedText {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = <String as serde::Deserialize>::deserialize(d)?;
        Ok(Self(zeroize::Zeroizing::new(value)))
    }
}
impl serde::Serialize for ProtectedText {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editing_preserves_utf8_and_clears_without_exposing_debug() {
        let mut s = SecretString::empty();
        s.push('é').unwrap();
        s.push('🙂').unwrap();
        assert_eq!(s.char_count(), 2);
        assert!(s.insert_str(1, "x").is_err());
        s.insert_str(2, "q").unwrap();
        s.remove(2).unwrap();
        assert!(s.pop());
        assert_eq!(s.char_count(), 1);
        assert_eq!(format!("{s:?}"), "SecretString(<redacted>)");
        s.clear();
        assert!(s.is_empty());
        s.push_str("ok").unwrap();
        assert_eq!(s.len(), 2);
    }
    #[test]
    fn overflow_and_zero_byte_do_not_change_the_secret() {
        let mut s = SecretString::empty();
        s.push_str("unchanged").unwrap();
        assert!(s.push_str(&"x".repeat(MAX_SECRET_BYTES)).is_err());
        assert!(s.push_str("\0").is_err());
        assert_eq!(s.expose_secret(), "unchanged");
    }
}
