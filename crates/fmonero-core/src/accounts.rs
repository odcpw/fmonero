//! Account/contact snapshots and stale-safe guarded mutations.
use crate::{
    rpc::{
        jsonrpc::Deadline,
        wallet::{AccountRow, BookEntry},
    },
    wallet_ops::{self, WalletIdentity},
    Context, Error, WalletClient,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub wallet: WalletIdentity,
    pub selected: u32,
    pub accounts: Vec<AccountRow>,
    pub contacts: Vec<BookEntry>,
}
pub fn snapshot(ctx: &Context, w: &WalletClient) -> Result<Snapshot, Error> {
    let _lock = crate::send::take_send_lock(ctx)?;
    let wallet = wallet_ops::identity(ctx, w)?;
    let accounts = w.get_accounts()?;
    let contacts = w.get_address_book()?;
    wallet_ops::verify(ctx, w, &wallet)?;
    Ok(Snapshot {
        wallet,
        selected: ctx.settings.wallet.account,
        accounts,
        contacts,
    })
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreatedAccount {
    pub account_index: u32,
    pub address: String,
}
pub fn create(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    label: &str,
) -> Result<CreatedAccount, Error> {
    wallet_ops::text(label)?;
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    w.call(
        "create_account",
        serde_json::json!({"label":label}),
        Deadline::Probe,
    )
}
pub fn rename(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    account: &AccountRow,
    label: &str,
) -> Result<(), Error> {
    wallet_ops::text(label)?;
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    verify_account(w, account)?;
    w.ack(
        "label_account",
        serde_json::json!({"account_index":account.account_index,"label":label}),
        Deadline::Probe,
    )
}
pub fn select(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    account: &AccountRow,
) -> Result<(), Error> {
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    verify_account(w, account)?;
    let mut settings = crate::Settings::load(&ctx.paths)?;
    settings.wallet.account = account.account_index;
    settings.save(ctx)
}
fn verify_account(w: &WalletClient, expected: &AccountRow) -> Result<(), Error> {
    if !w.get_accounts()?.iter().any(|a| {
        a.account_index == expected.account_index
            && a.base_address == expected.base_address
            && a.label == expected.label
    }) {
        return Err(wallet_ops::stale());
    }
    Ok(())
}
pub fn add_contact(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    address: &str,
    description: &str,
) -> Result<u64, Error> {
    validate_contact(wallet, address, description)?;
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    w.add_address_book(address, description)
}
pub fn edit_contact(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    expected: &BookEntry,
    address: &str,
    description: &str,
) -> Result<(), Error> {
    validate_contact(wallet, address, description)?;
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    verify_contact(w, expected)?;
    w.ack("edit_address_book",serde_json::json!({"index":expected.index,"set_address":true,"address":address,"set_description":true,"description":description}),Deadline::Probe)
}
pub fn remove_contact(
    ctx: &Context,
    w: &WalletClient,
    wallet: &WalletIdentity,
    expected: &BookEntry,
) -> Result<(), Error> {
    let _lock = wallet_ops::guard(ctx, w, wallet)?;
    verify_contact(w, expected)?;
    w.delete_address_book(expected.index)
}
fn verify_contact(w: &WalletClient, expected: &BookEntry) -> Result<(), Error> {
    if !w.get_address_book()?.iter().any(|a| {
        a.index == expected.index
            && a.address == expected.address
            && a.description == expected.description
    }) {
        return Err(wallet_ops::stale());
    }
    Ok(())
}
fn validate_contact(
    wallet: &WalletIdentity,
    address: &str,
    description: &str,
) -> Result<(), Error> {
    wallet_ops::text(description)?;
    if crate::address::parse(address)?.net != wallet.network {
        return Err(Error::invalid_address(
            "Choose an address on the open wallet's network.",
        ));
    }
    Ok(())
}
