//! Per-platform secret storage behind [`super::auth::KeychainSecrets`].

#[cfg(any(windows, test))]
mod dpapi_paths;
#[cfg(windows)]
pub(super) mod dpapi_secrets;
#[cfg(not(windows))]
pub(super) mod keyring_secrets;
#[cfg(target_os = "macos")]
pub(super) mod macos_queue;
pub(super) mod pending_deletes;

#[cfg(windows)]
pub(super) use dpapi_secrets as backend;
#[cfg(not(windows))]
pub(super) use keyring_secrets as backend;
