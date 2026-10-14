//! View-only creation and engine-native output/key-image exchange.
use crate::{
    artifacts::{self, ArtifactReceipt},
    restore::{RestoreState, Restored},
    rpc::jsonrpc::Deadline,
    secrets::{ProtectedText, SecretRef},
    wallet_ops::{self, WalletIdentity},
    CancelToken, Context, Error, SecretString, WalletClient,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub struct CreateViewOnly {
    pub name: String,
    pub address: String,
    pub view_key: SecretString,
    pub password: SecretString,
    pub restore_height: u64,
}
pub fn create(
    ctx: &Context,
    w: &WalletClient,
    request: CreateViewOnly,
    cancel: &CancelToken,
) -> Result<Restored, Error> {
    crate::wallet_dir::require_new(ctx, &request.name)?;
    let parsed = crate::address::parse(&request.address)?;
    if parsed.net != ctx.settings.node.network.net_type()
        || parsed.kind != crate::AddrKind::Standard
    {
        return Err(Error::invalid_address(
            "Use a standard primary address on the selected network.",
        ));
    }
    if request.password.is_empty()
        || request.view_key.len() != 64
        || !request
            .view_key
            .expose_secret()
            .bytes()
            .all(|b| b.is_ascii_hexdigit())
    {
        return Err(Error::usage(
            "Enter a private view key of 64 hexadecimal characters and a nonempty wallet password.",
        ));
    }
    let _lock = crate::send::take_send_lock(ctx)?;
    crate::send::refuse_if_send_in_flight(ctx)?;
    if cancel.is_cancelled() {
        return Err(Error::Interrupted);
    }
    let unit_started = crate::units::ensure_running(
        ctx,
        &ctx.settings.wallet.unit,
        &|| w.get_version().is_ok(),
        ctx.timeouts.unit_probe,
        cancel,
    )?;
    if cancel.is_cancelled() {
        return Err(Error::Interrupted);
    }
    crate::wallet_dir::require_new(ctx, &request.name)?;
    #[derive(Serialize)]
    struct Params<'a> {
        filename: &'a str,
        address: &'a str,
        spendkey: &'static str,
        viewkey: SecretRef<'a>,
        password: SecretRef<'a>,
        restore_height: u64,
        autosave_current: bool,
    }
    #[derive(Deserialize)]
    struct Reply {
        address: String,
    }
    crate::state::clear_last_open(ctx)?;
    let reply: Result<Reply, Error> = w.protected(
        "generate_from_keys",
        &Params {
            filename: &request.name,
            address: &request.address,
            spendkey: "",
            viewkey: SecretRef(request.view_key.expose_secret()),
            password: SecretRef(request.password.expose_secret()),
            restore_height: request.restore_height,
            autosave_current: true,
        },
        Deadline::Open,
    );
    let mut result = Restored {
        name: request.name.clone(),
        restore_height: request.restore_height,
        address: None,
        network: None,
        state: RestoreState::Unknown,
        warning: None,
        unit_started,
    };
    let expected = request.address.clone();
    drop(request);
    let reply = match reply {
        Ok(r) => r,
        Err(e) => {
            result.warning=Some(format!("View-only creation was not confirmed: {e} Inspect the wallet list before trying again."));
            return Ok(result);
        }
    };
    if reply.address != expected {
        return Err(Error::upstream(
            "The created view-only wallet returned a different primary address.",
        ));
    }
    result.address = Some(reply.address);
    result.network = Some(parsed.net);
    result.state = RestoreState::NeedsSetup;
    if cancel.is_cancelled() {
        result.warning =
            Some("The view-only wallet exists. Open it to complete node setup.".into());
        return Ok(result);
    }
    let mut fresh = ctx.clone();
    fresh.settings.wallet.account = 0;
    match crate::wallet_session::install_daemon_and_record(&fresh, w, &result.name) {
        Ok(_) => {
            result.state = RestoreState::Ready;
            if crate::capabilities::note(w, crate::capabilities::WalletKind::ViewOnly).is_err() {
                result.warning = Some(
                    "The view-only wallet is ready, but its type hint could not be stored.".into(),
                );
            }
        }
        Err(e) => {
            result.warning = Some(format!(
                "The view-only wallet exists, but setup did not finish: {e}"
            ))
        }
    };
    Ok(result)
}

#[derive(Serialize, Deserialize)]
struct OutputBundle {
    format: String,
    address: String,
    outputs_data_hex: ProtectedText,
}
#[derive(Serialize, Deserialize)]
struct SignedKeyImage {
    key_image: String,
    signature: String,
}
#[derive(Serialize, Deserialize)]
struct KeyImages {
    offset: u32,
    // Monero 0.18.5.3 omits this member when the exported vector is empty.
    // Offset remains required; malformed or null image lists are not empty exports.
    #[serde(default)]
    signed_key_images: Vec<SignedKeyImage>,
}
#[derive(Serialize, Deserialize)]
struct ImageBundle {
    format: String,
    address: String,
    images: KeyImages,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportedImages {
    pub height: u64,
    pub spent: crate::Piconero,
    pub unspent: crate::Piconero,
}
fn write_bundle<T: Serialize>(path: &Path, value: &T) -> Result<ArtifactReceipt, Error> {
    let mut buffer = crate::secrets::PrivateBuffer::new(artifacts::MAX_ARTIFACT_BYTES);
    serde_json::to_writer(&mut buffer, value)
        .map_err(|_| Error::usage("The exchange artifact exceeds its size limit."))?;
    artifacts::write_new(path, buffer.as_slice())
}
pub fn export_outputs(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    path: &Path,
) -> Result<ArtifactReceipt, Error> {
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    #[derive(Serialize)]
    struct Params {
        all: bool,
        start: u32,
        count: u32,
    }
    #[derive(Deserialize)]
    struct Reply {
        outputs_data_hex: ProtectedText,
    }
    let reply: Reply = w.protected(
        "export_outputs",
        &Params {
            all: true,
            start: 0,
            count: u32::MAX,
        },
        Deadline::Transfer,
    )?;
    validate_hex(&reply.outputs_data_hex.0)?;
    write_bundle(
        path,
        &OutputBundle {
            format: "fmonero.outputs/1".into(),
            address: wallet.primary_address.clone(),
            outputs_data_hex: reply.outputs_data_hex,
        },
    )
}
pub fn import_outputs(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    path: &Path,
) -> Result<u64, Error> {
    let bytes = artifacts::read(ctx, path)?;
    let bundle: OutputBundle = serde_json::from_slice(&bytes)
        .map_err(|_| Error::usage("This is not a bounded fmonero output-exchange artifact."))?;
    if bundle.format != "fmonero.outputs/1" || bundle.address != wallet.primary_address {
        return Err(Error::safety(
            "This output artifact belongs to another wallet or format.",
        ));
    }
    validate_hex(&bundle.outputs_data_hex.0)?;
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    #[derive(Serialize)]
    struct Params<'a> {
        outputs_data_hex: SecretRef<'a>,
    }
    #[derive(Deserialize)]
    struct Reply {
        num_imported: u64,
    }
    let reply: Reply = w.protected(
        "import_outputs",
        &Params {
            outputs_data_hex: SecretRef(&bundle.outputs_data_hex.0),
        },
        Deadline::Transfer,
    )?;
    Ok(reply.num_imported)
}
pub fn export_key_images(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    path: &Path,
) -> Result<ArtifactReceipt, Error> {
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    let images: KeyImages = w.protected(
        "export_key_images",
        &serde_json::json!({"all":true}),
        Deadline::Transfer,
    )?;
    validate_images(&images)?;
    write_bundle(
        path,
        &ImageBundle {
            format: "fmonero.key-images/1".into(),
            address: wallet.primary_address.clone(),
            images,
        },
    )
}
pub fn import_key_images(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    path: &Path,
) -> Result<ImportedImages, Error> {
    if !crate::transport::DaemonConnection::from_context(ctx)?.trusted() {
        return Err(Error::safety(
            "Importing key images requires an explicitly trusted daemon.",
        ));
    }
    let bytes = artifacts::read(ctx, path)?;
    let bundle: ImageBundle = serde_json::from_slice(&bytes)
        .map_err(|_| Error::usage("This is not a bounded fmonero key-image artifact."))?;
    if bundle.format != "fmonero.key-images/1" || bundle.address != wallet.primary_address {
        return Err(Error::safety(
            "This key-image artifact belongs to another wallet or format.",
        ));
    }
    validate_images(&bundle.images)?;
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    w.protected("import_key_images", &bundle.images, Deadline::Refresh)
}
fn validate_images(images: &KeyImages) -> Result<(), Error> {
    if images.signed_key_images.len() > 100_000 {
        return Err(Error::usage("The key-image artifact is too large."));
    }
    for image in &images.signed_key_images {
        if image.key_image.len() != 64 || image.signature.len() != 128 {
            return Err(Error::usage(
                "The key-image artifact contains invalid lengths.",
            ));
        }
        validate_hex(&image.key_image)?;
        validate_hex(&image.signature)?;
    }
    Ok(())
}
pub(crate) fn validate_hex(text: &str) -> Result<(), Error> {
    if text.is_empty()
        || text.len() > artifacts::MAX_ARTIFACT_BYTES
        || !text.len().is_multiple_of(2)
        || !text.bytes().all(|b| b.is_ascii_hexdigit())
    {
        Err(Error::usage(
            "The engine artifact is empty, malformed or exceeds its bound.",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_empty_key_image_response_accepts_omitted_vector() {
        // Exact empty-result shape reported by the isolated Monero 0.18.5.3 run.
        // This regression replays its schema; it does not execute a native wallet.
        for response in [r#"{"offset":0}"#, r#"{"offset":0,"signed_key_images":[]}"#] {
            let images: KeyImages = serde_json::from_str(response).unwrap();
            assert_eq!(images.offset, 0);
            assert!(images.signed_key_images.is_empty());
            validate_images(&images).unwrap();
            // Our exchange artifact is explicit even if the engine omitted the list.
            assert_eq!(
                serde_json::to_value(&images).unwrap(),
                serde_json::json!({"offset":0,"signed_key_images":[]})
            );
        }
        let later: KeyImages = serde_json::from_str(r#"{"offset":7}"#).unwrap();
        assert_eq!(
            later.offset, 7,
            "an empty list must not discard the engine offset"
        );
    }

    #[test]
    fn empty_key_image_compatibility_does_not_default_invalid_fields() {
        for response in [
            r#"{}"#,
            r#"{"signed_key_images":[]}"#,
            r#"{"offset":null}"#,
            r#"{"offset":-1}"#,
            r#"{"offset":0,"signed_key_images":null}"#,
            r#"{"offset":0,"signed_key_images":{}}"#,
            r#"{"offset":0,"signed_key_images":[{}]}"#,
        ] {
            assert!(
                serde_json::from_str::<KeyImages>(response).is_err(),
                "{response}"
            );
        }
    }
}
