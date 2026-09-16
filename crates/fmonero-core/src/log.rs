//! A bounded tail of a daemon log file (DESIGN.md §5b, §4.1).
//!
//! Signature-only at the Phase 1a gate (§8). Consumed by `node log` and by the TUI's Log
//! screen, which is why it is a core function rather than a subcommand body.

use std::path::{Path, PathBuf};

use crate::error::Error;

/// The lines a tail produced.
pub struct Tail {
    pub lines: Vec<String>,
    /// True when the read stopped at the 4 MiB bound rather than at the start of the file.
    pub truncated: bool,
    pub path: PathBuf,
}

/// The most this ever reads from the end of a log, however many lines were asked for.
pub const MAX_TAIL_BYTES: u64 = 4 * 1024 * 1024;

/// Bounded read of the last `lines` lines, with an optional substring filter.
///
/// A missing file is `Ok(Tail { lines: vec![], .. })`, **never an error**: `node log` with
/// no log yet is a GREEN row (§4b).
pub fn tail(_path: &Path, _lines: usize, _grep: Option<&str>) -> Result<Tail, Error> {
    Err(Error::unimplemented("log::tail"))
}
