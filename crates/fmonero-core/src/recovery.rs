//! Encrypted Monero-generated recovery wallets, never copies of an owner's wallet.
use crate::{
    artifacts::PrivateDirectory,
    native_process::{self, NativeProcess},
    rpc::jsonrpc::Deadline,
    secrets::{PrivateBuffer, ProtectedText, SecretRef},
    wallet_ops::{self, WalletIdentity},
    CancelToken, Context, Error, SecretString, WalletClient,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Clone, Debug, Serialize)]
pub struct RecoveryBackup {
    pub directory: PathBuf,
    pub wallet_file: PathBuf,
    pub address: String,
    pub restore_height: u64,
    pub labels_included: bool,
    pub history_included: bool,
    pub password_reopened: bool,
    pub coverage: String,
}
/// Explicit backup action: Monero returns its mnemonic into protected memory,
/// then a separate offline Monero process creates a newly encrypted wallet.
pub fn create(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    password: SecretString,
    restore_height: u64,
    destination: &Path,
    cancel: &CancelToken,
) -> Result<RecoveryBackup, Error> {
    if password.expose_secret().contains(['\r', '\n']) {
        return Err(Error::usage(
            "A native recovery wallet password must not contain line breaks.",
        ));
    }
    if password.is_empty() {
        return Err(Error::usage(
            "Choose a nonempty password for the recovery wallet.",
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| Error::usage("Choose an absolute destination directory."))?;
    let name = destination
        .file_name()
        .ok_or_else(|| Error::usage("Choose a new recovery directory name."))?;
    if !destination.is_absolute() || destination.exists() {
        return Err(Error::safety(
            "Choose a new absolute recovery directory. Existing destinations are never replaced.",
        ));
    }
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    if cancel.is_cancelled() {
        return Err(Error::Interrupted);
    }
    let stage = PrivateDirectory::new(parent)?;
    #[derive(Deserialize)]
    struct Reply {
        key: ProtectedText,
    }
    let seed: Reply = w.protected(
        "query_key",
        &serde_json::json!({"key_type":"mnemonic"}),
        Deadline::Open,
    )?;
    if seed.key.0.is_empty() || seed.key.0.len() > crate::secrets::MAX_SECRET_BYTES {
        return Err(Error::upstream(
            "Monero did not provide a supported deterministic recovery seed.",
        ));
    }
    #[derive(Serialize)]
    struct Generation<'a> {
        version: u32,
        filename: &'static str,
        scan_from_height: u64,
        password: SecretRef<'a>,
        seed: SecretRef<'a>,
        create_address_file: u32,
    }
    let mut input = PrivateBuffer::new(64 * 1024);
    serde_json::to_writer(
        &mut input,
        &Generation {
            version: 1,
            filename: "recovery",
            scan_from_height: restore_height,
            password: SecretRef(password.expose_secret()),
            seed: SecretRef(&seed.key.0),
            create_address_file: 0,
        },
    )
    .map_err(|_| Error::failure("Protected recovery input could not be encoded."))?;
    let memory = native_process::sealed_input(input.as_slice())?;
    let password_input = native_process::sealed_input(password.expose_secret().as_bytes())?;
    drop(input);
    drop(seed);
    drop(password);
    let mut args = native_process::base_args(wallet.network);
    args.extend([
        "--offline".into(),
        "--generate-from-json".into(),
        "/proc/self/fd/3".into(),
        "--shared-ringdb-dir".into(),
        "ringdb".into(),
        "--command".into(),
        "address".into(),
    ]);
    let mut process = NativeProcess::start(&args, &stage.path(), Some(&memory), ctx.timeouts.open)?;
    process.wait(cancel)?;
    if !process.success() || !process.public_addresses().contains(&wallet.primary_address) {
        return Err(Error::backend(
            "Monero did not confirm an encrypted recovery wallet with the expected address.",
        ));
    }
    drop(process);
    drop(memory);
    // A separate offline engine must reopen the encrypted artifact with the
    // chosen password and reproduce the identity before it is published.
    let mut reopen_args = native_process::base_args(wallet.network);
    reopen_args.extend([
        "--offline".into(),
        "--wallet-file".into(),
        "recovery".into(),
        "--password-file".into(),
        "/proc/self/fd/3".into(),
        "--shared-ringdb-dir".into(),
        "ringdb".into(),
        "--command".into(),
        "address".into(),
    ]);
    let mut check = NativeProcess::start(
        &reopen_args,
        &stage.path(),
        Some(&password_input),
        ctx.timeouts.open,
    )?;
    check.wait(cancel)?;
    if !check.success() || !check.public_addresses().contains(&wallet.primary_address) {
        return Err(Error::backend("Monero could not reopen the encrypted recovery wallet with the chosen password and expected identity."));
    }
    drop(check);
    // Inspect only new artifact metadata; never read or copy wallet bytes.
    for filename in ["recovery", "recovery.keys"] {
        let path = stage.path().join(filename);
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|_| Error::backend("Monero did not create the complete recovery wallet."))?;
        if !meta.is_file()
            || meta.len() == 0
            || meta.len() > crate::artifacts::MAX_ARTIFACT_BYTES as u64
        {
            return Err(Error::backend(
                "The generated recovery artifact is incomplete or exceeds its bound.",
            ));
        }
        use std::os::unix::fs::PermissionsExt;
        // Newly generated artifact only. Monero remains the writer of its contents.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(|_| {
            Error::failure("Recovery artifact permissions could not be restricted.")
        })?;
        std::fs::File::open(&path)
            .and_then(|f| f.sync_all())
            .map_err(|_| Error::failure("The recovery artifact could not be made durable."))?;
    }
    let directory = stage.publish(name)?;
    Ok(RecoveryBackup{wallet_file:directory.join("recovery"),directory,address:wallet.primary_address.clone(),restore_height,labels_included:false,history_included:false,password_reopened:true,coverage:"Encrypted deterministic recovery wallet. Rescanning reconstructs chain transactions; account/subaddress labels, address book, notes and local history metadata are not included. Record any used account/subaddress indices and rescan from an appropriate height.".into()})
}
