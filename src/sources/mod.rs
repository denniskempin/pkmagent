//! Collect sources. Category plans fill in the live backends.

use chrono::{DateTime, FixedOffset, NaiveDate};

use crate::contacts_index::ContactIndex;
use crate::timeutil::Slice;

pub mod calendar;
pub mod contacts;
pub mod gmail;
pub mod messages;

#[derive(Debug, Clone)]
pub struct SourceError {
    pub reason: String,
}

impl SourceError {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for SourceError {}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub filename: Option<String>,
    pub media_type: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CollectedItem {
    pub stable_id: String,
    pub display_title: String,
    pub empty_title_fallback: String,
    pub source_url: String,
    pub t: DateTime<FixedOffset>,
    pub tie_break: String,
    pub file_day: NaiveDate,
    pub extra_front_matter: String,
    pub body: String,
    pub attachments: Vec<Attachment>,
}

pub trait CollectSource: Send + Sync {
    fn fetch(
        &self,
        slice: &Slice,
        contacts: &ContactIndex,
    ) -> Result<Vec<CollectedItem>, SourceError>;

    fn group_extra_front_matter(&self, items: &[CollectedItem]) -> String {
        items
            .first()
            .map(|item| item.extra_front_matter.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
pub fn test_item(stable_id: &str, title: &str, t: DateTime<FixedOffset>) -> CollectedItem {
    CollectedItem {
        stable_id: stable_id.to_string(),
        display_title: title.to_string(),
        empty_title_fallback: "untitled".to_string(),
        source_url: format!("https://example.test/{stable_id}"),
        t,
        tie_break: String::new(),
        file_day: t.date_naive(),
        extra_front_matter: String::new(),
        body: format!("{title}\n"),
        attachments: Vec::new(),
    }
}

#[cfg(test)]
pub struct MapSource {
    pub calls: std::sync::Arc<std::sync::Mutex<Vec<NaiveDate>>>,
    pub items: Vec<CollectedItem>,
    pub error: Option<String>,
    pub fail_on: Option<NaiveDate>,
}

#[cfg(test)]
impl MapSource {
    pub fn new(items: Vec<CollectedItem>) -> Self {
        Self {
            calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            items,
            error: None,
            fail_on: None,
        }
    }

    pub fn failing(day: NaiveDate, reason: &str) -> Self {
        Self {
            error: Some(reason.to_string()),
            fail_on: Some(day),
            ..Self::new(Vec::new())
        }
    }
}

#[cfg(test)]
impl CollectSource for MapSource {
    fn fetch(
        &self,
        slice: &Slice,
        _contacts: &ContactIndex,
    ) -> Result<Vec<CollectedItem>, SourceError> {
        self.calls.lock().expect("calls").push(slice.day);
        let fail =
            self.fail_on == Some(slice.day) || (self.fail_on.is_none() && self.error.is_some());
        if fail {
            return Err(SourceError::new(
                self.error.clone().unwrap_or_else(|| "boom".to_string()),
            ));
        }
        Ok(self.items.clone())
    }
}
