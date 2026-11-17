use crate::account::AccountId;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub fn account_dir(root: &Path, account: &AccountId) -> PathBuf {
    // Hash account IDs to preserve case distinctions on Windows and avoid reserved names.
    root.join(format!("{:x}", Sha256::digest(account.as_str().as_bytes())))
}

pub fn secret_path(root: &Path, key: &str) -> Result<PathBuf, String> {
    let (account, name) = key
        .strip_prefix("connector:")
        .and_then(|key| key.split_once(':'))
        .filter(|(_, name)| !name.is_empty())
        .ok_or("invalid account-scoped secret key")?;
    let account = AccountId::parse(account)?;

    let filename = format!("{:x}", Sha256::digest(name.as_bytes()));
    Ok(account_dir(root, &account).join(filename))
}

pub fn delete_account_secrets(root: &Path, account: &AccountId) -> Result<(), String> {
    match std::fs::remove_dir_all(account_dir(root, account)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not delete account credentials: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_remain_separate_on_case_insensitive_filesystems() {
        let root = Path::new("secrets");
        let first = secret_path(root, "connector:Account:mcp-slack:token").unwrap();
        let second = secret_path(root, "connector:account:mcp-slack:token").unwrap();
        let other_secret = secret_path(root, "connector:Account:proton_mail:password").unwrap();

        assert_ne!(first.parent(), second.parent());
        assert_eq!(first.file_name(), second.file_name());
        assert_eq!(first.parent(), other_secret.parent());
        assert_ne!(first.file_name(), other_secret.file_name());
        assert_eq!(first.parent().unwrap().parent(), Some(root));
        for path in [&first, &second, &other_secret] {
            for component in path.strip_prefix(root).unwrap().components() {
                let component = component.as_os_str().to_str().unwrap();
                assert_eq!(component.len(), 64);
                assert!(component
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)));
            }
        }
    }

    #[test]
    fn secret_names_cannot_escape_the_account_directory() {
        let root = Path::new("secrets");
        let normal = secret_path(root, "connector:account:token").unwrap();
        let unusual = secret_path(root, "connector:account:../../CON\\token:value").unwrap();

        assert_eq!(normal.parent(), unusual.parent());
        for key in [
            "account:token",
            "connector:../account:token",
            "connector::token",
            "connector:account:",
        ] {
            assert!(secret_path(root, key).is_err());
        }
    }

    #[test]
    fn deleting_account_secrets_preserves_other_accounts_and_legacy_files() {
        let dir = tempfile::tempdir().unwrap();
        let account = AccountId::parse("Account").unwrap();
        let first = secret_path(dir.path(), "connector:Account:token").unwrap();
        let second = secret_path(dir.path(), "connector:account:token").unwrap();
        for path in [&first, &second] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"encrypted secret").unwrap();
        }
        let legacy = dir.path().join("legacy-file");
        std::fs::write(&legacy, b"legacy secret").unwrap();

        delete_account_secrets(dir.path(), &account).unwrap();

        assert!(!first.exists());
        assert_eq!(std::fs::read(second).unwrap(), b"encrypted secret");
        assert_eq!(std::fs::read(legacy).unwrap(), b"legacy secret");
        delete_account_secrets(dir.path(), &account).unwrap();
    }

    #[test]
    fn legacy_files_are_not_selected_or_modified() {
        let dir = tempfile::tempdir().unwrap();
        let key = "connector:account:mcp-slack:token";
        let legacy = dir
            .path()
            .join(format!("{:x}", Sha256::digest(key.as_bytes())));
        std::fs::write(&legacy, b"legacy encrypted secret").unwrap();

        let path = secret_path(dir.path(), key).unwrap();

        assert_ne!(path, legacy);
        assert!(!path.exists());
        assert_eq!(std::fs::read(legacy).unwrap(), b"legacy encrypted secret");
    }
}
