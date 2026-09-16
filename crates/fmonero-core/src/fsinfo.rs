//! Free space, measured locally (DESIGN.md §5b, §7.1).
//!
//! Signature-only at the Phase 1a gate (§8).

use std::path::Path;

use crate::error::Error;

/// Free bytes on the filesystem holding `path` (`statvfs`).
///
/// The doctor's `disk_space` measures through this and **never** reads
/// `get_info.free_space`, which a restricted node reports as `2^64-1` (§1b.3, §7.1).
pub fn free_bytes(path: &Path) -> Result<u64, Error> {
    use std::os::unix::ffi::OsStrExt;
    // `statvfs` needs a path that exists: the data dir may not, so walk up to the nearest
    // ancestor that does. The answer is about a **filesystem**, and every ancestor of a
    // not-yet-created directory is on the one it will be created on.
    let mut probe = path;
    while !probe.exists() {
        match probe.parent() {
            Some(p) => probe = p,
            None => {
                return Err(Error::failure(format!(
                    "{} has no existing ancestor to measure",
                    path.display()
                )))
            }
        }
    }
    let mut c = Vec::with_capacity(probe.as_os_str().as_bytes().len() + 1);
    c.extend_from_slice(probe.as_os_str().as_bytes());
    c.push(0);
    // SAFETY: `c` is a NUL-terminated path and `buf` is a valid, fully written-to statvfs.
    let (rc, buf) = unsafe {
        let mut buf: libc::statvfs = std::mem::zeroed();
        let rc = libc::statvfs(c.as_ptr() as *const libc::c_char, &mut buf);
        (rc, buf)
    };
    if rc != 0 {
        return Err(Error::failure(format!(
            "statvfs({}) failed: {}",
            probe.display(),
            std::io::Error::last_os_error()
        )));
    }
    // `f_bavail` is what an unprivileged process may actually use, which is the number a disk
    // floor is about; `f_bfree` includes the root reserve.
    Ok(buf.f_bavail.saturating_mul(buf.f_frsize))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_space_is_measured_locally_and_works_for_a_path_that_does_not_exist_yet() {
        let here = free_bytes(&std::env::temp_dir()).expect("a real filesystem");
        assert!(here > 0, "a writable temp dir has some space");
        // The doctor measures `node.data_dir`, which may not exist on a fresh machine.
        let future = std::env::temp_dir().join("fmonero-not-created-yet/lmdb");
        let answer = free_bytes(&future).expect("the nearest existing ancestor is measured");
        assert!(answer > 0);
        // And never `get_info.free_space`, which a restricted node reports as 2^64-1 (§7.1).
        assert!(answer < u64::MAX);
    }
}
