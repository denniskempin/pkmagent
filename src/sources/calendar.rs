//! Calendar. Plan 004 replaces the EventKit fetch. Mapping stays available on every target.

#![allow(dead_code)]

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use crate::contacts_index::ContactIndex;
use crate::sources::{CollectSource, CollectedItem, SourceError};
use crate::timeutil::Slice;

pub struct CalendarSource {
    pub zone: Tz,
}

pub struct EventRecord {}

pub struct AttendeeRecord {}

pub fn predicate_interval(slice: &Slice) -> (DateTime<Utc>, DateTime<Utc>) {
    (
        slice.start.with_timezone(&Utc),
        slice.end.with_timezone(&Utc),
    )
}

pub fn map_events(
    _events: &[EventRecord],
    _slice: &Slice,
    _zone: Tz,
    _contacts: &ContactIndex,
) -> Vec<CollectedItem> {
    Vec::new()
}

impl CollectSource for CalendarSource {
    fn fetch(
        &self,
        _slice: &Slice,
        _contacts: &ContactIndex,
    ) -> Result<Vec<CollectedItem>, SourceError> {
        let _ = self.zone;
        Err(SourceError::new("Calendar is only available on macOS"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_fetch_requires_macos() {
        if cfg!(target_os = "macos") {
            return;
        }
        let source = CalendarSource {
            zone: chrono_tz::UTC,
        };
        let slice = crate::timeutil::whole_day_slice(
            chrono::NaiveDate::from_ymd_opt(2026, 9, 26).unwrap(),
            chrono_tz::UTC,
        );
        let err = source
            .fetch(&slice, &crate::contacts_index::ContactIndex::empty())
            .unwrap_err();
        assert_eq!(err.reason, "Calendar is only available on macOS");
    }
}
