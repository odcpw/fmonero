//! Free space, measured locally (DESIGN.md §5b, §7.1).
//!
//! Signature-only at the Phase 1a gate (§8).

use std::path::Path;

use crate::error::Error;

/// Free bytes on the filesystem holding `path` (`statvfs`).
///
/// The doctor's `disk_space` measures through this and **never** reads
/// `get_info.free_space`, which a restricted node reports as `2^64-1` (§1b.3, §7.1).
pub fn free_bytes(_path: &Path) -> Result<u64, Error> {
    Err(Error::unimplemented("fsinfo::free_bytes"))
}
