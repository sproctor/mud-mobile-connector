//! OS keychain storage for secrets: the MUD Mobile device token (`wlk_…`) and the
//! per-account play.net passwords. Security invariant #3: secrets live here, never in
//! config. All operations degrade gracefully — on a system with no available credential
//! store (e.g. headless Linux without a running Secret Service) the getters return
//! `None` and the UI re-prompts.

use crate::error::{AppError, AppResult};

const SERVICE: &str = "mudmobile-connector";
const ACCOUNT: &str = "device-token";

fn entry() -> AppResult<keyring::Entry> {
    keyring::Entry::new(SERVICE, ACCOUNT)
        .map_err(|e| AppError::Other(format!("keychain unavailable: {e}")))
}

/// Fetch the stored token, or `None` if absent / the keychain is unavailable.
pub fn get_token() -> Option<String> {
    entry().ok()?.get_password().ok()
}

/// Store (or replace) the token.
pub fn set_token(token: &str) -> AppResult<()> {
    entry()?
        .set_password(token)
        .map_err(|e| AppError::Other(format!("storing token: {e}")))
}

/// Remove the stored token. Succeeds if it was already absent.
pub fn delete_token() -> AppResult<()> {
    let entry = entry()?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(AppError::Other(format!("deleting token: {e}"))),
    }
}

/// Keychain entry for a play.net account's password. The account name is normalized
/// (trimmed + lowercased) so case differences between a saved profile and a typed
/// account map to the same stored password.
fn password_entry(account: &str) -> AppResult<keyring::Entry> {
    let user = format!("password:{}", account.trim().to_ascii_lowercase());
    keyring::Entry::new(SERVICE, &user)
        .map_err(|e| AppError::Other(format!("keychain unavailable: {e}")))
}

/// Fetch the stored play.net password for an account, if any.
pub fn get_password(account: &str) -> Option<String> {
    password_entry(account).ok()?.get_password().ok()
}

/// Store (or replace) the play.net password for an account.
pub fn set_password(account: &str, password: &str) -> AppResult<()> {
    password_entry(account)?
        .set_password(password)
        .map_err(|e| AppError::Other(format!("storing password: {e}")))
}

/// Remove the stored password for an account. Succeeds if it was already absent.
pub fn delete_password(account: &str) -> AppResult<()> {
    let entry = password_entry(account)?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(AppError::Other(format!("deleting password: {e}"))),
    }
}
