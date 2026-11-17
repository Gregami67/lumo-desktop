//! Credential Manager caps a secret at 2560 bytes, which OAuth credentials exceed, so on Windows
//! each secret is a DPAPI-encrypted file readable only by the current Windows user.
//! Legacy flat files are not read or migrated; existing connectors must be reconnected.

use crate::reporting::credential_failure;
use crate::settings::KEYCHAIN_SERVICE;
use std::io::ErrorKind;
use std::path::PathBuf;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

fn secrets_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|dir| PathBuf::from(dir).join(KEYCHAIN_SERVICE).join("secrets"))
}

fn secret_path(key: &str) -> Result<PathBuf, String> {
    let dir = secrets_dir().ok_or("secure storage is unavailable: LOCALAPPDATA is not set")?;
    super::dpapi_paths::secret_path(&dir, key)
}

fn blob_of(bytes: &[u8]) -> Result<CRYPT_INTEGER_BLOB, String> {
    Ok(CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len()).map_err(|_| "secret is too large".to_string())?,
        pbData: bytes.as_ptr().cast_mut(),
    })
}

/// Copies out a buffer DPAPI allocated, then frees it.
fn take_blob(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    let bytes = unsafe { std::slice::from_raw_parts(blob.pbData, blob.cbData as usize) }.to_vec();
    unsafe { LocalFree(Some(HLOCAL(blob.pbData.cast()))) };
    bytes
}

fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
    let input = blob_of(plain)?;
    let mut output = CRYPT_INTEGER_BLOB::default();
    let result = unsafe {
        CryptProtectData(
            &input,
            PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    result.map_err(|e| format!("could not encrypt secret: {e}"))?;
    Ok(take_blob(output))
}

fn unprotect(encrypted: &[u8]) -> Result<Vec<u8>, String> {
    let input = blob_of(encrypted)?;
    let mut output = CRYPT_INTEGER_BLOB::default();
    let result = unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    result.map_err(|e| format!("could not decrypt secret: {e}"))?;
    Ok(take_blob(output))
}

pub fn store_secret_blocking(key: &str, secret: &str) -> Result<(), String> {
    let path = secret_path(key)?;
    let encrypted = protect(secret.as_bytes())?;
    let tmp = path.with_extension("tmp");
    path.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&tmp, encrypted))
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| format!("could not store secret: {e}"))
}

pub fn load_secret_blocking(key: &str) -> Result<Option<String>, String> {
    let encrypted = match std::fs::read(secret_path(key)?) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("could not load secret: {e}")),
    };
    String::from_utf8(unprotect(&encrypted)?)
        .map(Some)
        .inspect_err(|_| credential_failure("Credential decoding failed"))
        .map_err(|e| format!("could not load secret: {e}"))
}

pub fn delete_secret_blocking(key: &str) -> Result<(), String> {
    match std::fs::remove_file(secret_path(key)?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not delete secret: {e}")),
    }
}

pub fn delete_account_secrets_blocking(account: &crate::account::AccountId) -> Result<(), String> {
    let dir = secrets_dir().ok_or("secure storage is unavailable: LOCALAPPDATA is not set")?;
    super::dpapi_paths::delete_account_secrets(&dir, account)
}

pub fn secure_storage_available_blocking() -> bool {
    secrets_dir().is_some()
}

#[cfg(test)]
mod tests {
    use super::{protect, unprotect};

    #[test]
    fn a_secret_longer_than_credential_manager_allows_round_trips() {
        let secret = "x".repeat(10_000);

        let encrypted = protect(secret.as_bytes()).unwrap();

        assert_ne!(encrypted, secret.as_bytes());
        assert_eq!(unprotect(&encrypted).unwrap(), secret.as_bytes());
    }
}
