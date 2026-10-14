//! Bounded, private, atomic no-clobber artifacts. Never use this for wallet files.
use crate::{Context, Error};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

pub const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct ArtifactReceipt {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

fn refused() -> Error {
    Error::safety("Choose a new artifact filename in an existing directory you own. Existing files and symbolic links are never replaced.")
        .with_hint("fmonero wallet list")
}

/// Atomically publishes a new mode-0600 file, preserving every existing target.
/// openat/renameat2 anchor the entire operation to the same open directory.
pub fn write_new(path: &Path, bytes: &[u8]) -> Result<ArtifactReceipt, Error> {
    if bytes.len() > MAX_ARTIFACT_BYTES || !path.is_absolute() {
        return Err(refused());
    }
    let parent = path.parent().ok_or_else(refused)?;
    let name = path.file_name().ok_or_else(refused)?;
    let name = CString::new(name.as_bytes()).map_err(|_| refused())?;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(parent)
        .map_err(|e| Error::io("open", "the artifact directory", parent, &e))?;
    let metadata = directory.metadata().map_err(|_| refused())?;
    // SAFETY: geteuid has no preconditions.
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
        return Err(refused());
    }
    let temporary = CString::new(format!(".fmonero-{}.tmp", crate::rpc::random_hex(16)?))
        .map_err(|_| refused())?;
    let dir = directory.as_raw_fd();
    // SAFETY: both C strings are NUL-terminated and directory owns dir.
    let fd = unsafe {
        libc::openat(
            dir,
            temporary.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(Error::io(
            "create",
            "the private artifact",
            path,
            &std::io::Error::last_os_error(),
        ));
    }
    // SAFETY: successful openat transfers this fresh descriptor to File.
    let mut file = unsafe { File::from_raw_fd(fd) };
    let result = (|| {
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| Error::io("write", "the private artifact", path, &e))?;
        // SAFETY: descriptors and C strings remain valid; NOREPLACE is atomic.
        let rc = unsafe {
            libc::renameat2(
                dir,
                temporary.as_ptr(),
                dir,
                name.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if rc != 0 {
            return Err(refused());
        }
        directory
            .sync_all()
            .map_err(|e| Error::io("sync", "the artifact directory", parent, &e))?;
        Ok(ArtifactReceipt {
            path: path.to_owned(),
            bytes: bytes.len() as u64,
            sha256: hex(&Sha256::digest(bytes)),
        })
    })();
    // Cleanup only our random temporary name; never remove the destination.
    // SAFETY: directory and temporary remain valid, including after rename.
    unsafe {
        libc::unlinkat(dir, temporary.as_ptr(), 0);
    }
    result
}

/// Only typed import workflows may read an artifact. Exclude configured wallet
/// directories and wallet/key companions before opening; no wallet bytes are read.
pub(crate) fn read(ctx: &Context, path: &Path) -> Result<Zeroizing<Vec<u8>>, Error> {
    if !path.is_absolute() || path.extension().is_some_and(|e| e == "keys") {
        return Err(refused());
    }
    let canonical = path
        .canonicalize()
        .map_err(|e| Error::io("inspect", "the input artifact", path, &e))?;
    let wallet_dir = ctx
        .settings
        .wallet
        .dir
        .canonicalize()
        .unwrap_or_else(|_| ctx.settings.wallet.dir.clone());
    if canonical.starts_with(&wallet_dir)
        || path.with_extension("keys").exists()
        || PathBuf::from(format!("{}.keys", path.display())).exists()
    {
        return Err(refused());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| Error::io("open", "the input artifact", path, &e))?;
    // Validate the opened descriptor too: a parent path may have changed between
    // canonicalization and open. Do this before the first byte is read.
    let opened =
        std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).map_err(|_| refused())?;
    if opened != canonical
        || opened.starts_with(&wallet_dir)
        || PathBuf::from(format!("{}.keys", opened.display())).exists()
    {
        return Err(refused());
    }
    let meta = file.metadata().map_err(|_| refused())?;
    if !meta.is_file() || meta.len() > MAX_ARTIFACT_BYTES as u64 {
        return Err(refused());
    }
    // Fixed capacity avoids freeing partial sensitive input on growth.
    let mut bytes = Zeroizing::new(vec![0; MAX_ARTIFACT_BYTES + 1]);
    let mut used = 0;
    loop {
        let n = file
            .read(&mut bytes[used..])
            .map_err(|e| Error::io("read", "the input artifact", path, &e))?;
        if n == 0 {
            break;
        }
        used += n;
        if used > MAX_ARTIFACT_BYTES {
            return Err(refused());
        }
    }
    bytes.truncate(used);
    Ok(bytes)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

/// A new private output directory anchored to its parent. Only Monero-created
/// recovery/hardware staging uses this; existing wallets never enter this helper.
pub(crate) struct PrivateDirectory {
    parent: File,
    directory: File,
    temporary: CString,
    parent_path: PathBuf,
    published: bool,
}
impl PrivateDirectory {
    pub(crate) fn new(parent_path: &Path) -> Result<Self, Error> {
        if !parent_path.is_absolute() {
            return Err(refused());
        }
        let parent = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(parent_path)
            .map_err(|_| refused())?;
        let metadata = parent.metadata().map_err(|_| refused())?;
        // SAFETY: geteuid has no preconditions.
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
            return Err(refused());
        }
        let temporary =
            CString::new(format!(".fmonero-private-{}", crate::rpc::random_hex(16)?)).unwrap();
        // SAFETY: live parent fd and valid C string.
        if unsafe { libc::mkdirat(parent.as_raw_fd(), temporary.as_ptr(), 0o700) } != 0 {
            return Err(refused());
        }
        // SAFETY: freshly created directory within the owned parent.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                temporary.as_ptr(),
                libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_RDONLY,
            )
        };
        if fd < 0 {
            return Err(refused());
        }
        // SAFETY: fresh descriptor transferred exactly once.
        let directory = unsafe { File::from_raw_fd(fd) };
        Ok(Self {
            parent,
            directory,
            temporary,
            parent_path: parent_path.into(),
            published: false,
        })
    }
    pub(crate) fn path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/self/fd/{}", self.directory.as_raw_fd()))
    }
    pub(crate) fn publish(mut self, name: &std::ffi::OsStr) -> Result<PathBuf, Error> {
        let destination = CString::new(name.as_bytes()).map_err(|_| refused())?;
        if name.as_bytes().contains(&b'/') || name.as_bytes() == b"." || name.as_bytes() == b".." {
            return Err(refused());
        }
        self.directory.sync_all().map_err(|_| refused())?;
        // SAFETY: both names refer to entries beneath the same held directory.
        if unsafe {
            libc::renameat2(
                self.parent.as_raw_fd(),
                self.temporary.as_ptr(),
                self.parent.as_raw_fd(),
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } != 0
        {
            return Err(refused());
        }
        self.published = true;
        self.parent.sync_all().map_err(|_| {
            Error::failure(
                "The generated directory was published but its durability could not be confirmed.",
            )
        })?;
        Ok(self.parent_path.join(name))
    }
}
impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        if !self.published {
            // Only the fresh random staging directory is removed. Anchor traversal to
            // parent fd so a renamed parent cannot redirect cleanup to another wallet.
            let path = PathBuf::from(format!("/proc/self/fd/{}", self.parent.as_raw_fd()))
                .join(std::ffi::OsStr::from_bytes(self.temporary.as_bytes()));
            let _ = std::fs::remove_dir_all(path);
        }
    }
}
impl PrivateDirectory {
    pub(crate) fn write(&self, name: &str, bytes: &[u8]) -> Result<(), Error> {
        if name.contains('/') || name == "." || name == ".." || bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(refused());
        }
        let name = CString::new(name).map_err(|_| refused())?;
        // SAFETY: held directory descriptor and bounded fixed filename.
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_WRONLY | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(refused());
        }
        // SAFETY: new descriptor transferred once.
        let mut file = unsafe { File::from_raw_fd(fd) };
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| refused())
    }
    pub(crate) fn read_generated(&self, name: &str) -> Result<Zeroizing<Vec<u8>>, Error> {
        if name.contains('/') || name == "." || name == ".." {
            return Err(refused());
        }
        let name = CString::new(name).map_err(|_| refused())?;
        // SAFETY: held directory descriptor; only new non-wallet transaction artifacts.
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_NOFOLLOW | libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if fd < 0 {
            return Err(refused());
        }
        // SAFETY: new descriptor transferred once.
        let mut file = unsafe { File::from_raw_fd(fd) };
        let meta = file.metadata().map_err(|_| refused())?;
        if !meta.is_file() || meta.len() > MAX_ARTIFACT_BYTES as u64 {
            return Err(refused());
        }
        let mut out = Zeroizing::new(vec![0; MAX_ARTIFACT_BYTES + 1]);
        let mut used = 0;
        loop {
            let n = file.read(&mut out[used..]).map_err(|_| refused())?;
            if n == 0 {
                break;
            }
            used += n;
            if used > MAX_ARTIFACT_BYTES {
                return Err(refused());
            }
        }
        out.truncate(used);
        Ok(out)
    }
}
pub(crate) fn decode_hex(text: &str) -> Result<Zeroizing<Vec<u8>>, Error> {
    crate::watch::validate_hex(text)?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(text.len() / 2));
    for pair in text.as_bytes().as_chunks::<2>().0 {
        let nibble = |b: u8| {
            if b <= b'9' {
                b - b'0'
            } else {
                b.to_ascii_lowercase() - b'a' + 10
            }
        };
        bytes.push(nibble(pair[0]) * 16 + nibble(pair[1]));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, DirBuilderExt, PermissionsExt};
    #[test]
    fn publication_is_private_and_never_clobbers_files_or_links() {
        let root = std::env::temp_dir().join(format!(
            "fmonero-artifact-{}-{}",
            std::process::id(),
            crate::rpc::random_hex(8).unwrap()
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        let path = root.join("history.json");
        let receipt = write_new(&path, b"one").unwrap();
        assert_eq!(receipt.bytes, 3);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(write_new(&path, b"two").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"one");
        let link = root.join("link");
        symlink(&path, &link).unwrap();
        assert!(write_new(&link, b"three").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"one");
        std::fs::remove_dir_all(root).unwrap();
    }
}
