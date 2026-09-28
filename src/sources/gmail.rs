//! Gmail. Plan 002 replaces the fetch and authorize bodies.

use std::path::Path;

use chrono_tz::Tz;

use crate::contacts_index::ContactIndex;
use crate::sources::{CollectSource, CollectedItem, SourceError};
use crate::timeutil::Slice;

#[derive(Debug)]
pub struct GmailAuthError {
    pub reason: String,
}

impl std::fmt::Display for GmailAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for GmailAuthError {}

pub struct GmailSource {
    pub secrets_dir: std::path::PathBuf,
    pub zone: Tz,
}

impl CollectSource for GmailSource {
    fn fetch(
        &self,
        _slice: &Slice,
        _contacts: &ContactIndex,
    ) -> Result<Vec<CollectedItem>, SourceError> {
        let _ = (self.secrets_dir.as_path(), self.zone);
        Err(SourceError::new("Gmail source is not implemented"))
    }
}

pub fn authorize(_secrets_dir: &Path) -> Result<(), GmailAuthError> {
    Err(GmailAuthError {
        reason: "Gmail authorization is not implemented".to_string(),
    })
}
