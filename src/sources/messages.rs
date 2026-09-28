//! Messages. Plan 003 replaces the fetch body. The live database open is macOS-only.

use chrono_tz::Tz;

use crate::contacts_index::ContactIndex;
use crate::sources::{CollectSource, CollectedItem, SourceError};
use crate::timeutil::Slice;

pub struct MessagesSource {
    pub zone: Tz,
}

impl CollectSource for MessagesSource {
    fn fetch(
        &self,
        _slice: &Slice,
        _contacts: &ContactIndex,
    ) -> Result<Vec<CollectedItem>, SourceError> {
        let _ = self.zone;
        Err(SourceError::new("Messages is only available on macOS"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_fetch_requires_macos() {
        if cfg!(target_os = "macos") {
            return;
        }
        let source = MessagesSource {
            zone: chrono_tz::UTC,
        };
        let slice = crate::timeutil::whole_day_slice(
            chrono::NaiveDate::from_ymd_opt(2026, 9, 26).unwrap(),
            chrono_tz::UTC,
        );
        let err = source
            .fetch(&slice, &crate::contacts_index::ContactIndex::empty())
            .unwrap_err();
        assert_eq!(err.reason, "Messages is only available on macOS");
    }
}
