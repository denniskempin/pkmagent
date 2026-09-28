//! Contacts import. Plan 005 replaces the snapshot and the create-versus-refresh writer.

#![allow(dead_code)]

use std::path::Path;

use crate::sources::SourceError;

#[derive(Clone, Debug)]
pub struct ContactRecord {
    pub id: String,
    pub display_title: String,
    pub source_url: String,
    pub extra_front_matter: String,
    pub note_body: String,
}

pub fn fetch_snapshot() -> Result<Vec<ContactRecord>, SourceError> {
    #[cfg(target_os = "macos")]
    {
        Err(SourceError::new("contacts fetch is not implemented"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(SourceError::new("Contacts is only available on macOS"))
    }
}

pub fn import_contacts(
    contacts_dir: &Path,
    snapshot: Result<Vec<ContactRecord>, SourceError>,
    imported_at: &str,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<u64, SourceError> {
    let _ = (contacts_dir, imported_at, progress);
    let _records = snapshot?;
    Err(SourceError::new("contacts import is not implemented"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempVault;

    #[test]
    fn import_error_does_not_create_directory() {
        let vault = TempVault::new();
        let dir = vault.root.join("contacts");
        let err = import_contacts(
            &dir,
            Err(SourceError::new("access denied")),
            "2026-09-27T10:00:00-07:00",
            &mut |_, _, _| {},
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "access denied");
        assert!(!dir.exists());
    }

    #[test]
    fn contacts_snapshot_requires_macos() {
        if cfg!(target_os = "macos") {
            return;
        }
        let err = fetch_snapshot().unwrap_err();
        assert_eq!(err.reason, "Contacts is only available on macOS");
    }
}
