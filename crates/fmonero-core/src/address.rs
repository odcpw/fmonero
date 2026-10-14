//! Offline Monero address decoding and Keccak checksum validation.
//!
//! Every send still validates twice: locally, then against the open Monero wallet.
//!
//! `PREFIXES` is the **only** place an address form is described. Adding a Carrot form
//! after the FCMP++ fork is one row here plus one object in
//! `tests/fixtures/address-forms.json`; a unit test asserts the two have equal length, so
//! adding one without the other fails the build (§1b.1).

use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};

use crate::error::Error;

/// Monero's base58 alphabet — `src/common/base58.cpp:46`.
const ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
/// Encoded characters per decoded byte count — `src/common/base58.cpp:48`. A full block
/// is 8 bytes → 11 characters; a trailing 5-byte block → 7 characters.
const ENCODED_BLOCK_SIZES: [usize; 9] = [0, 2, 3, 5, 6, 7, 9, 10, 11];
const FULL_BLOCK_BYTES: usize = 8;
const FULL_BLOCK_CHARS: usize = 11;

/// Which network an address belongs to.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetType {
    Mainnet,
    Testnet,
    Stagenet,
}

impl NetType {
    /// The wire spelling, which is also what `get_info.nettype` returns (§1b.3).
    pub fn as_str(self) -> &'static str {
        match self {
            NetType::Mainnet => "mainnet",
            NetType::Testnet => "testnet",
            NetType::Stagenet => "stagenet",
        }
    }
}

impl std::fmt::Display for NetType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which kind of address, which decides its length and its prefix.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AddrKind {
    Standard,
    Integrated,
    Subaddress,
}

impl AddrKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AddrKind::Standard => "standard",
            AddrKind::Integrated => "integrated",
            AddrKind::Subaddress => "subaddress",
        }
    }
}

impl AddrKind {
    /// The kind as a **person** reads it (§2o.2: a subaddress is an `address`, and `standard` is
    /// upstream's word for "not one of the other two"). `as_str` stays the wire value that
    /// `wallet validate --json` carries; this is what the Send screen says under the field
    /// (self-read, round 3: it read `mainnet standard`).
    pub fn words(self) -> &'static str {
        match self {
            AddrKind::Standard | AddrKind::Subaddress => "address",
            AddrKind::Integrated => "address with a payment id",
        }
    }
}

impl std::fmt::Display for AddrKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One (network, kind) row: its prefix varint, the base58 digits its first block can
/// start with, and its character length.
#[derive(Copy, Clone, Debug, Serialize)]
pub struct AddrForm {
    pub net: NetType,
    pub kind: AddrKind,
    pub prefix: u64,
    /// The base58 digits the first 8-byte block can start with. Usually one; testnet
    /// standard (prefix 53) is `'9'` or `'A'`, because the block value
    /// `[p<<56, (p<<56)|0x00FF_FFFF_FFFF_FFFF]` straddles a digit boundary (§1b.8).
    pub leading: &'static [char],
    pub len: usize,
}

/// A standard or subaddress payload: 1 prefix byte + 32 spend + 32 view + 4 checksum
/// = 69 bytes → 8×11 + 7 = **95** characters (§1b.8).
const LEN_STANDARD: usize = 95;
/// An integrated address adds an 8-byte payment id = 77 bytes → 9×11 + 7 = **106**.
const LEN_INTEGRATED: usize = 106;

/// The ONLY place an address form is described (§1b.1, §5b). Prefixes from
/// `src/cryptonote_config.h:227-229` (mainnet), `:270-272` (testnet), `:285-287`
/// (stagenet); leading digits computed over Monero's alphabet (§1b.8).
pub const PREFIXES: &[AddrForm] = &[
    AddrForm {
        net: NetType::Mainnet,
        kind: AddrKind::Standard,
        prefix: 18,
        leading: &['4'],
        len: LEN_STANDARD,
    },
    AddrForm {
        net: NetType::Mainnet,
        kind: AddrKind::Integrated,
        prefix: 19,
        leading: &['4'],
        len: LEN_INTEGRATED,
    },
    AddrForm {
        net: NetType::Mainnet,
        kind: AddrKind::Subaddress,
        prefix: 42,
        leading: &['8'],
        len: LEN_STANDARD,
    },
    AddrForm {
        net: NetType::Testnet,
        kind: AddrKind::Standard,
        prefix: 53,
        // Two digits: prefix 53's first block spans a base58 boundary (§1b.8), so a hard
        // one-character check would reject roughly one testnet address in five.
        leading: &['9', 'A'],
        len: LEN_STANDARD,
    },
    AddrForm {
        net: NetType::Testnet,
        kind: AddrKind::Integrated,
        prefix: 54,
        leading: &['A'],
        len: LEN_INTEGRATED,
    },
    AddrForm {
        net: NetType::Testnet,
        kind: AddrKind::Subaddress,
        prefix: 63,
        leading: &['B'],
        len: LEN_STANDARD,
    },
    AddrForm {
        net: NetType::Stagenet,
        kind: AddrKind::Standard,
        prefix: 24,
        leading: &['5'],
        len: LEN_STANDARD,
    },
    AddrForm {
        net: NetType::Stagenet,
        kind: AddrKind::Integrated,
        prefix: 25,
        leading: &['5'],
        len: LEN_INTEGRATED,
    },
    AddrForm {
        net: NetType::Stagenet,
        kind: AddrKind::Subaddress,
        prefix: 36,
        leading: &['7'],
        len: LEN_STANDARD,
    },
];

/// An address with a valid supported prefix, payload length and Keccak checksum.
#[derive(Clone, Debug, Serialize)]
pub struct Address {
    pub text: String,
    pub net: NetType,
    pub kind: AddrKind,
}

/// §2o rule 10: name the thing the user typed, then what a good one looks like. `why` is a
/// whole sentence, so the two read as two sentences rather than as a field and its value.
fn invalid(text: &str, why: impl AsRef<str>) -> Error {
    Error::invalid_address(format!(
        "\"{text}\" is not a Monero address. {}",
        why.as_ref()
    ))
    .with_hint("fmonero wallet validate <address>")
}

/// Decode Monero's block-wise base58. `None` when a character is outside the alphabet,
/// when a block length is impossible, or when a block's value does not fit its bytes.
fn base58_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 8 / FULL_BLOCK_CHARS + 8);
    let full_blocks = bytes.len() / FULL_BLOCK_CHARS;
    let last = bytes.len() % FULL_BLOCK_CHARS;
    for i in 0..full_blocks {
        let block = &bytes[i * FULL_BLOCK_CHARS..(i + 1) * FULL_BLOCK_CHARS];
        out.extend_from_slice(&decode_block(block, FULL_BLOCK_BYTES)?);
    }
    if last > 0 {
        let bytes_out = ENCODED_BLOCK_SIZES.iter().position(|n| *n == last)?;
        let block = &bytes[full_blocks * FULL_BLOCK_CHARS..];
        out.extend_from_slice(&decode_block(block, bytes_out)?);
    }
    Some(out)
}

/// One base58 block as a big-endian integer in exactly `bytes_out` bytes.
fn decode_block(block: &[u8], bytes_out: usize) -> Option<Vec<u8>> {
    let mut value: u128 = 0;
    for c in block {
        let digit = ALPHABET.iter().position(|a| a == c)? as u128;
        value = value.checked_mul(58)?.checked_add(digit)?;
    }
    if bytes_out < 16 && value >= 1u128 << (8 * bytes_out as u32) {
        return None; // overflows its block, as upstream's decoder also refuses
    }
    Some(value.to_be_bytes()[16 - bytes_out..].to_vec())
}

/// Read a Monero varint (7 bits per byte, little-endian groups).
fn read_varint(bytes: &[u8]) -> Option<u64> {
    let mut value: u64 = 0;
    for (i, b) in bytes.iter().take(10).enumerate() {
        value |= u64::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// Structural parse (§1b.8). **Not authoritative**: every send also calls
/// `WalletClient::validate_address` (§2a.6), which is the answer that decides.
pub fn parse(s: &str) -> Result<Address, Error> {
    if s.is_empty() {
        return Err(invalid("", "You typed nothing."));
    }
    if s.len() != LEN_STANDARD && s.len() != LEN_INTEGRATED {
        return Err(invalid(
            s,
            format!(
                // Rule 1: a semicolon joining two facts is two sentences.
                "Addresses are {LEN_STANDARD} characters and start with 4 or 8. This one is {}.",
                s.len()
            ),
        ));
    }
    if let Some(c) = s
        .chars()
        .find(|c| !c.is_ascii() || !ALPHABET.contains(&(*c as u8)))
    {
        return Err(invalid(
            s,
            format!("\"{c}\" is not a character Monero addresses use. 0, O, I and l never are."),
        ));
    }
    // The fast reject of §1b.8: the first character is the base58 digit of the whole
    // first 8-byte block, so it is a *set* per form. Used before the decode, and only
    // against the union for this length — never as the thing that names the form.
    let first = s.chars().next().unwrap_or(' ');
    if !PREFIXES
        .iter()
        .filter(|f| f.len == s.len())
        .any(|f| f.leading.contains(&first))
    {
        return Err(invalid(
            s,
            format!(
                "no Monero address of {} characters starts with '{first}'",
                s.len()
            ),
        ));
    }
    let decoded = base58_decode(s)
        .ok_or_else(|| invalid(s, "Those characters do not decode to an address."))?;
    let prefix = read_varint(&decoded)
        .filter(|_| decoded[0] & 0x80 == 0)
        .ok_or_else(|| invalid(s, "It does not have a canonical address prefix."))?;
    let Some(form) = PREFIXES.iter().find(|f| f.prefix == prefix) else {
        return Err(invalid(
            s,
            format!("It does not start like a Monero address (prefix {prefix})."),
        ));
    };
    if form.len != s.len() {
        return Err(invalid(
            s,
            format!(
                "That start means a {} {} address, which is {} characters, not {}.",
                form.net,
                form.kind,
                form.len,
                s.len()
            ),
        ));
    }
    let payload_len = if form.kind == AddrKind::Integrated {
        73
    } else {
        65
    };
    if decoded.len() != payload_len + 4 {
        return Err(invalid(s, "The decoded address has an invalid length."));
    }
    let checksum = Keccak256::digest(&decoded[..payload_len]);
    if decoded[payload_len..] != checksum[..4] {
        return Err(invalid(
            s,
            "Its checksum is wrong. Check the complete address.",
        ));
    }
    Ok(Address {
        text: s.to_string(),
        net: form.net,
        kind: form.kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The encoder, needed only to build the nine synthetic forms this test file checks.
    /// Production code never encodes an address.
    fn base58_encode(bytes: &[u8]) -> String {
        let mut out = String::new();
        let full = bytes.len() / FULL_BLOCK_BYTES;
        let last = bytes.len() % FULL_BLOCK_BYTES;
        for i in 0..full {
            out.push_str(&encode_block(
                &bytes[i * FULL_BLOCK_BYTES..(i + 1) * FULL_BLOCK_BYTES],
            ));
        }
        if last > 0 {
            out.push_str(&encode_block(&bytes[full * FULL_BLOCK_BYTES..]));
        }
        out
    }

    fn encode_block(block: &[u8]) -> String {
        let mut value: u128 = 0;
        for b in block {
            value = value * 256 + u128::from(*b);
        }
        let chars = ENCODED_BLOCK_SIZES[block.len()];
        let mut buf = vec!['1'; chars];
        for i in (0..chars).rev() {
            buf[i] = ALPHABET[(value % 58) as usize] as char;
            value /= 58;
        }
        buf.into_iter().collect()
    }

    /// A synthetic address of `form`, with `fill` in every payload byte.
    fn synthetic(form: &AddrForm, fill: u8) -> String {
        let payload_len = if form.len == LEN_INTEGRATED { 77 } else { 69 };
        let mut bytes = vec![form.prefix as u8];
        bytes.extend(std::iter::repeat_n(fill, payload_len - 5));
        let checksum = Keccak256::digest(&bytes);
        bytes.extend_from_slice(&checksum[..4]);
        base58_encode(&bytes)
    }

    #[test]
    fn the_table_has_the_nine_forms_of_the_design() {
        assert_eq!(PREFIXES.len(), 9);
        let expect = [
            (NetType::Mainnet, AddrKind::Standard, 18u64, LEN_STANDARD),
            (NetType::Mainnet, AddrKind::Integrated, 19, LEN_INTEGRATED),
            (NetType::Mainnet, AddrKind::Subaddress, 42, LEN_STANDARD),
            (NetType::Testnet, AddrKind::Standard, 53, LEN_STANDARD),
            (NetType::Testnet, AddrKind::Integrated, 54, LEN_INTEGRATED),
            (NetType::Testnet, AddrKind::Subaddress, 63, LEN_STANDARD),
            (NetType::Stagenet, AddrKind::Standard, 24, LEN_STANDARD),
            (NetType::Stagenet, AddrKind::Integrated, 25, LEN_INTEGRATED),
            (NetType::Stagenet, AddrKind::Subaddress, 36, LEN_STANDARD),
        ];
        for (row, (net, kind, prefix, len)) in PREFIXES.iter().zip(expect) {
            assert_eq!(
                (row.net, row.kind, row.prefix, row.len),
                (net, kind, prefix, len)
            );
            assert!(!row.leading.is_empty());
        }
    }

    /// `tests/fixtures/address-forms.json` is written by Phase 0 in parallel with this
    /// phase, so an absent file **skips with a message**; a present one must have exactly
    /// as many objects as `PREFIXES` has rows (§1b.1, §8 Phase 0).
    #[test]
    fn prefixes_and_the_fixture_have_the_same_row_count() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/address-forms.json"
        );
        let Ok(text) = std::fs::read_to_string(path) else {
            // Written through `Write` rather than a print macro, so that §2g's grep — "no
            // print macro anywhere in crates/fmonero-core/src" — stays literally true and
            // cannot be weakened later to exempt tests.
            use std::io::Write;
            let _ = writeln!(
                std::io::stderr(),
                "SKIP: {path} does not exist yet (DESIGN.md §8 Phase 0 writes it); \
                 PREFIXES has {} rows and this assertion runs as soon as the fixture lands",
                PREFIXES.len()
            );
            return;
        };
        let value: serde_json::Value =
            serde_json::from_str(&text).expect("address-forms.json must be valid JSON");
        let rows = value
            .as_array()
            .map(|a| a.len())
            .or_else(|| {
                value
                    .get("forms")
                    .and_then(|f| f.as_array())
                    .map(|a| a.len())
            })
            .expect("address-forms.json must be an array of forms (or {\"forms\": [...]})");
        assert_eq!(
            rows,
            PREFIXES.len(),
            "address::PREFIXES and tests/fixtures/address-forms.json must describe the same forms"
        );
        for row in value.get("forms").and_then(|f| f.as_array()).unwrap() {
            let parsed = parse(row["example"].as_str().unwrap()).unwrap();
            assert_eq!(serde_json::to_value(parsed.net).unwrap(), row["net"]);
            assert_eq!(serde_json::to_value(parsed.kind).unwrap(), row["kind"]);
        }
    }

    #[test]
    fn every_form_round_trips_through_a_synthetic_address() {
        for form in PREFIXES {
            for fill in [0x00, 0xff, 0x42] {
                let text = synthetic(form, fill);
                assert_eq!(text.len(), form.len, "{form:?} produced {text}");
                let a = parse(&text).unwrap_or_else(|e| panic!("{form:?}: {e}"));
                assert_eq!((a.net, a.kind), (form.net, form.kind), "{text}");
                assert!(
                    form.leading.contains(&text.chars().next().unwrap()),
                    "{text} starts outside {:?}",
                    form.leading
                );
            }
        }
    }

    /// The case a one-character check gets wrong: testnet standard (prefix 53) spans two
    /// base58 digits, and both must parse (§1b.8).
    #[test]
    fn testnet_standard_starts_with_either_nine_or_a() {
        let form = PREFIXES
            .iter()
            .find(|f| f.net == NetType::Testnet && f.kind == AddrKind::Standard)
            .expect("testnet standard");
        assert_eq!(form.leading, &['9', 'A']);
        let low = synthetic(form, 0x00);
        let high = synthetic(form, 0xff);
        assert_eq!(low.chars().next(), Some('9'), "{low}");
        assert_eq!(high.chars().next(), Some('A'), "{high}");
        for text in [low, high] {
            let a = parse(&text).expect("both leading digits are valid testnet addresses");
            assert_eq!((a.net, a.kind), (NetType::Testnet, AddrKind::Standard));
        }
    }

    /// A real mainnet address: upstream's donation address
    /// (`src/simplewallet/simplewallet.h:56`), which is a **subaddress** (prefix 42).
    #[test]
    fn a_real_mainnet_address_parses() {
        let addr = "888tNkZrPN6JsEgekjMnABU4TBzc2Dt29EPAvkRxbANsAnjyPbb3iQ1YBRk1UXcdRsiKc9dhwMVgN5S9cQUiyoogDavup3H";
        assert_eq!(addr.len(), LEN_STANDARD);
        let a = parse(addr).expect("the donation address is a valid mainnet subaddress");
        assert_eq!(a.net, NetType::Mainnet);
        assert_eq!(a.kind, AddrKind::Subaddress);
        assert_eq!(a.text, addr);
    }

    #[test]
    fn keccak_is_not_sha3_and_a_public_address_checksum_mutation_fails() {
        // Public Keccak-256 empty-input vector, independent of address encoding.
        let expected = [
            0xc5, 0xd2, 0x46, 0x01, 0x86, 0xf7, 0x23, 0x3c, 0x92, 0x7e, 0x7d, 0xb2, 0xdc, 0xc7,
            0x03, 0xc0, 0xe5, 0x00, 0xb6, 0x53, 0xca, 0x82, 0x27, 0x3b, 0x7b, 0xfa, 0xd8, 0x04,
            0x5d, 0x85, 0xa4, 0x70,
        ];
        assert_eq!(&Keccak256::digest([])[..], &expected);
        let good = "888tNkZrPN6JsEgekjMnABU4TBzc2Dt29EPAvkRxbANsAnjyPbb3iQ1YBRk1UXcdRsiKc9dhwMVgN5S9cQUiyoogDavup3H";
        let mut wrong = good.to_owned();
        wrong.replace_range(wrong.len() - 1.., "J");
        assert!(parse(good).is_ok());
        assert_eq!(parse(&wrong).unwrap_err().exit_code(), 15);
        for form in PREFIXES {
            let mut bytes = base58_decode(&synthetic(form, 0x42)).unwrap();
            bytes[4] ^= 1;
            assert!(parse(&base58_encode(&bytes)).is_err());
        }
    }

    #[test]
    fn malformed_addresses_are_exit_fifteen() {
        let good = "888tNkZrPN6JsEgekjMnABU4TBzc2Dt29EPAvkRxbANsAnjyPbb3iQ1YBRk1UXcdRsiKc9dhwMVgN5S9cQUiyoogDavup3H";
        let mut too_short = good.to_string();
        too_short.pop();
        let mut bad_char = good.to_string();
        bad_char.replace_range(10..11, "0"); // 0 is not in Monero's alphabet
        let mut wrong_leading = good.to_string();
        wrong_leading.replace_range(0..1, "z");
        // A 95-character string whose prefix byte is not one of the nine.
        let unknown_prefix = {
            let mut bytes = vec![99u8];
            bytes.extend(std::iter::repeat_n(0u8, 68));
            base58_encode(&bytes)
        };
        for (text, why) in [
            ("", "empty"),
            (too_short.as_str(), "94 characters"),
            (bad_char.as_str(), "a '0'"),
            (wrong_leading.as_str(), "an impossible leading digit"),
            (unknown_prefix.as_str(), "prefix 99"),
        ] {
            let e = parse(text).unwrap_err();
            assert_eq!(e.exit_code(), 15, "{why}: {text}");
            assert!(!e.hint().is_empty());
        }
    }

    #[test]
    fn the_wire_spellings_are_lowercase() {
        assert_eq!(serde_json::to_value(NetType::Mainnet).unwrap(), "mainnet");
        assert_eq!(serde_json::to_value(NetType::Stagenet).unwrap(), "stagenet");
        assert_eq!(
            serde_json::to_value(AddrKind::Subaddress).unwrap(),
            "subaddress"
        );
        assert_eq!(
            serde_json::to_value(AddrKind::Integrated).unwrap(),
            "integrated"
        );
    }
}
