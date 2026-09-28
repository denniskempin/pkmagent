//! Calendar. EventKit runs only on macOS. Mapping a fixture event is pure.

use chrono::{DateTime, FixedOffset, NaiveDate, SecondsFormat, Utc};
use chrono_tz::Tz;

use crate::contacts_index::ContactIndex;
use crate::note;
use crate::sources::{CollectSource, CollectedItem, SourceError};
use crate::timeutil::{self, Slice};

pub struct CalendarSource {
    pub zone: Tz,
}

/// One occurrence, already expanded. Fixtures and the live EventKit path both fill this.
#[derive(Clone, Debug)]
pub struct EventRecord {
    pub event_identifier: String,
    pub calendar_item_identifier: String,
    pub calendar: String,
    pub title: Option<String>,
    pub all_day: bool,
    /// Event-zone start. Set for timed events.
    pub start: Option<DateTime<FixedOffset>>,
    /// Event-zone end. Set for timed events.
    pub end: Option<DateTime<FixedOffset>>,
    pub displayed_start: Option<NaiveDate>,
    pub exclusive_end: Option<NaiveDate>,
    pub ek_start_utc: Option<DateTime<Utc>>,
    pub location: Option<String>,
    pub notes: Option<String>,
    pub url: Option<String>,
    pub event_status: String,
    pub user_status: Option<String>,
    pub attendees: Vec<AttendeeRecord>,
}

#[derive(Clone, Debug)]
pub struct AttendeeRecord {
    pub name: String,
    pub email: String,
    pub status: String,
}

/// Pad the slice by 14 hours on each side so an all-day UTC midnight still overlaps.
pub fn predicate_interval(slice: &Slice) -> (DateTime<Utc>, DateTime<Utc>) {
    let pad = chrono::Duration::hours(14);
    (
        slice.start.with_timezone(&Utc) - pad,
        slice.end.with_timezone(&Utc) + pad,
    )
}

/// Gregorian year, month, and day of an all-day `startDate` read in UTC.
pub fn displayed_all_day_date(start_utc: DateTime<Utc>) -> NaiveDate {
    start_utc.date_naive()
}

pub fn map_events(
    events: &[EventRecord],
    slice: &Slice,
    zone: Tz,
    contacts: &ContactIndex,
) -> Vec<CollectedItem> {
    let mut items = Vec::new();
    for event in events {
        if event.event_identifier.is_empty() || event.calendar_item_identifier.is_empty() {
            continue;
        }
        let Some(times) = mapped_times(event, zone) else {
            continue;
        };
        if times.t < slice.start || times.t >= slice.end {
            continue;
        }
        let attendees = written_attendees(&event.attendees, contacts);
        let source_url = format!(
            "ical://ekevent/{}/{}?method=show&options=more",
            times.utc_stamp,
            percent_encode(&event.calendar_item_identifier)
        );
        let status = event_front_matter_status(&event.event_status, event.user_status.as_deref());
        items.push(CollectedItem {
            stable_id: format!("{}/{}", event.event_identifier, times.stable_start),
            display_title: display_title(event.title.as_deref()),
            empty_title_fallback: "untitled".to_string(),
            source_url: source_url.clone(),
            t: times.t,
            tie_break: String::new(),
            file_day: times.file_day,
            extra_front_matter: render_front_matter(
                &event.calendar,
                status,
                times.all_day,
                &times.start_text,
                &times.end_text,
                event.location.as_deref(),
                &attendees,
            ),
            body: render_body(
                event.notes.as_deref(),
                event.url.as_deref(),
                &source_url,
                &attendees,
            ),
            attachments: Vec::new(),
        });
    }
    items
}

struct MappedTimes {
    t: DateTime<FixedOffset>,
    file_day: NaiveDate,
    all_day: bool,
    start_text: String,
    end_text: String,
    stable_start: String,
    utc_stamp: String,
}

fn mapped_times(event: &EventRecord, zone: Tz) -> Option<MappedTimes> {
    if event.all_day {
        let displayed = event
            .displayed_start
            .or_else(|| event.ek_start_utc.map(displayed_all_day_date))?;
        let exclusive = event
            .exclusive_end
            .unwrap_or_else(|| displayed.succ_opt().unwrap_or(displayed));
        let inclusive = if exclusive > displayed {
            exclusive.pred_opt().unwrap_or(displayed)
        } else {
            displayed
        };
        let start_text = displayed.format("%Y-%m-%d").to_string();
        return Some(MappedTimes {
            t: timeutil::local_midnight(displayed, zone),
            file_day: displayed,
            all_day: true,
            end_text: inclusive.format("%Y-%m-%d").to_string(),
            stable_start: start_text.clone(),
            utc_stamp: format!("{}T000000Z", displayed.format("%Y%m%d")),
            start_text,
        });
    }

    let start = event.start?;
    let end = event.end.unwrap_or(start);
    let t = start.with_timezone(&zone).fixed_offset();
    let start_text = format_event_time(start);
    Some(MappedTimes {
        t,
        file_day: t.date_naive(),
        all_day: false,
        end_text: format_event_time(end),
        stable_start: start_text.clone(),
        utc_stamp: start
            .with_timezone(&Utc)
            .format("%Y%m%dT%H%M%SZ")
            .to_string(),
        start_text,
    })
}

fn format_event_time(dt: DateTime<FixedOffset>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, false)
}

fn display_title(title: Option<&str>) -> String {
    match title {
        Some(text) if !text.trim().is_empty() => text.to_string(),
        _ => String::new(),
    }
}

struct WrittenAttendee {
    name: String,
    email: String,
    status: &'static str,
    matched: bool,
}

fn written_attendees(
    attendees: &[AttendeeRecord],
    contacts: &ContactIndex,
) -> Vec<WrittenAttendee> {
    let mut rows: Vec<WrittenAttendee> = attendees
        .iter()
        .map(|attendee| {
            let matched_title = contacts.title_for_email(&attendee.email);
            WrittenAttendee {
                name: matched_title
                    .map(str::to_string)
                    .unwrap_or_else(|| attendee.name.clone()),
                email: attendee.email.clone(),
                status: attendee_status(&attendee.status),
                matched: matched_title.is_some(),
            }
        })
        .collect();
    rows.sort_by(|left, right| {
        left.email
            .as_bytes()
            .cmp(right.email.as_bytes())
            .then_with(|| left.name.as_bytes().cmp(right.name.as_bytes()))
    });
    rows
}

fn event_front_matter_status(event_status: &str, user_status: Option<&str>) -> &'static str {
    if event_status == "canceled" {
        return "cancelled";
    }
    match user_status {
        Some("accepted") => "accepted",
        Some("declined") => "declined",
        Some("tentative") => "tentative",
        Some("pending") => "needs_action",
        Some("unknown" | "delegated" | "completed" | "in_process") => "unknown",
        Some(_) => "unknown",
        None => match event_status {
            "confirmed" => "accepted",
            "tentative" => "tentative",
            _ => "unknown",
        },
    }
}

fn attendee_status(status: &str) -> &'static str {
    match status {
        "accepted" => "accepted",
        "declined" => "declined",
        "tentative" => "tentative",
        "pending" => "needs_action",
        _ => "unknown",
    }
}

fn render_front_matter(
    calendar: &str,
    status: &str,
    all_day: bool,
    start: &str,
    end: &str,
    location: Option<&str>,
    attendees: &[WrittenAttendee],
) -> String {
    let mut text = String::new();
    text.push_str(&format!(
        "calendar: {}\n",
        note::yaml_double_quoted(calendar)
    ));
    text.push_str(&format!("status: {status}\n"));
    text.push_str(&format!(
        "all_day: {}\n",
        if all_day { "true" } else { "false" }
    ));
    text.push_str(&format!("start: {start}\n"));
    text.push_str(&format!("end: {end}\n"));
    if let Some(location) = location.map(str::trim).filter(|value| !value.is_empty()) {
        text.push_str(&format!(
            "location: {}\n",
            note::yaml_double_quoted(location)
        ));
    }
    if !attendees.is_empty() {
        text.push_str("attendees:\n");
        for attendee in attendees {
            text.push_str(&format!(
                "  - name: {}\n",
                note::yaml_double_quoted(&attendee.name)
            ));
            text.push_str(&format!("    email: {}\n", yaml_email(&attendee.email)));
            text.push_str(&format!("    status: {}\n", attendee.status));
        }
    }
    text
}

fn yaml_email(email: &str) -> String {
    if email_is_plain_scalar(email) {
        email.to_string()
    } else {
        note::yaml_double_quoted(email)
    }
}

fn email_is_plain_scalar(email: &str) -> bool {
    let mut parts = email.split('@');
    let (Some(local), Some(domain)) = (parts.next(), parts.next()) else {
        return false;
    };
    if parts.next().is_some() || local.is_empty() || domain.is_empty() {
        return false;
    }
    local.bytes().all(is_email_local) && domain.bytes().all(is_email_domain)
}

fn is_email_local(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'%' | b'+' | b'-')
}

fn is_email_domain(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')
}

fn render_body(
    notes: Option<&str>,
    url: Option<&str>,
    source_url: &str,
    attendees: &[WrittenAttendee],
) -> String {
    let mut sections = Vec::new();
    if let Some(description) = notes.map(str::trim).filter(|text| !text.is_empty()) {
        sections.push(description.to_string());
    }
    let lines: Vec<String> = attendees.iter().filter_map(attendee_line).collect();
    if !lines.is_empty() {
        let mut block = String::from("## Attendees\n\n");
        block.push_str(&lines.join("\n"));
        sections.push(block);
    }
    if let Some(url) = url.map(str::trim).filter(|text| !text.is_empty()) {
        if url != source_url {
            sections.push(format!("URL: {url}"));
        }
    }
    if sections.is_empty() {
        return String::new();
    }
    let mut body = sections.join("\n\n");
    body.push('\n');
    body
}

fn attendee_line(attendee: &WrittenAttendee) -> Option<String> {
    if attendee.name.is_empty() && attendee.email.is_empty() {
        return None;
    }
    let status = attendee.status;
    if attendee.email.is_empty() {
        let who = if attendee.matched {
            format!("[[{}]]", attendee.name)
        } else {
            attendee.name.clone()
        };
        return Some(format!("- {who} ({status})"));
    }
    if attendee.name.is_empty() {
        return Some(format!("- <{}> ({status})", attendee.email));
    }
    let who = if attendee.matched {
        format!("[[{}]]", attendee.name)
    } else {
        attendee.name.clone()
    };
    Some(format!("- {who} <{}> ({status})", attendee.email))
}

/// Percent-encode every byte outside RFC 3986 unreserved (`A-Z a-z 0-9 - . _ ~`).
fn percent_encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    out
}

impl CollectSource for CalendarSource {
    fn fetch(
        &self,
        slice: &Slice,
        contacts: &ContactIndex,
    ) -> Result<Vec<CollectedItem>, SourceError> {
        #[cfg(target_os = "macos")]
        {
            let records = live::fetch_records(slice, self.zone)?;
            return Ok(map_events(&records, slice, self.zone, contacts));
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (slice, contacts, self.zone);
            Err(SourceError::new("Calendar is only available on macOS"))
        }
    }
}

#[cfg(target_os = "macos")]
mod live {
    use std::sync::mpsc;

    use block2::RcBlock;
    use chrono::{DateTime, FixedOffset, Utc};
    use chrono_tz::Tz;
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, NSObjectProtocol};
    use objc2::{msg_send, sel, AnyThread};
    use objc2_event_kit::{
        EKAuthorizationStatus, EKCalendar, EKEntityType, EKEvent, EKEventStatus, EKEventStore,
        EKParticipantStatus,
    };
    use objc2_foundation::{NSArray, NSDate, NSError, NSURL};

    use super::{displayed_all_day_date, predicate_interval, AttendeeRecord, EventRecord};
    use crate::sources::SourceError;
    use crate::timeutil::Slice;

    pub(super) fn fetch_records(slice: &Slice, zone: Tz) -> Result<Vec<EventRecord>, SourceError> {
        let store = unsafe { EKEventStore::init(EKEventStore::alloc()) };
        ensure_full_access(&store)?;
        let calendars = visible_calendars(&store)?;
        if calendars.is_empty() {
            return Ok(Vec::new());
        }
        let filtered = NSArray::from_retained_slice(&calendars);
        let (start, end) = predicate_interval(slice);
        let start_date = NSDate::dateWithTimeIntervalSince1970(unix_seconds(start));
        let end_date = NSDate::dateWithTimeIntervalSince1970(unix_seconds(end));
        let predicate = unsafe {
            store.predicateForEventsWithStartDate_endDate_calendars(
                &start_date,
                &end_date,
                Some(&filtered),
            )
        };
        let events: Option<Retained<NSArray<EKEvent>>> =
            unsafe { msg_send![&store, eventsMatchingPredicate: &*predicate] };
        let Some(events) = events else {
            return Err(SourceError::new("fetch failed"));
        };
        let mut records = Vec::new();
        for event in events.iter() {
            if let Some(record) = record_from_event(&event, zone) {
                records.push(record);
            }
        }
        Ok(records)
    }

    fn ensure_full_access(store: &EKEventStore) -> Result<(), SourceError> {
        let status = current_status();
        if status_allows_read(status) {
            return Ok(());
        }
        if status == EKAuthorizationStatus::Restricted {
            return Err(SourceError::new("access restricted"));
        }
        if status == EKAuthorizationStatus::Denied {
            return Err(SourceError::new("access denied"));
        }
        if status != EKAuthorizationStatus::NotDetermined
            && status != EKAuthorizationStatus::WriteOnly
        {
            return Err(SourceError::new("access denied"));
        }
        if !request_full_access(store) {
            return Err(SourceError::new("access denied"));
        }
        unsafe { store.reset() };
        if status_allows_read(current_status()) {
            Ok(())
        } else {
            Err(SourceError::new("access denied"))
        }
    }

    fn current_status() -> EKAuthorizationStatus {
        unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) }
    }

    #[allow(deprecated)]
    fn status_allows_read(status: EKAuthorizationStatus) -> bool {
        status == EKAuthorizationStatus::FullAccess || status == EKAuthorizationStatus::Authorized
    }

    #[allow(deprecated)]
    fn request_full_access(store: &EKEventStore) -> bool {
        let (tx, rx) = mpsc::channel();
        let block = RcBlock::new(move |granted: Bool, error: *mut NSError| {
            let ok = granted.as_bool() && error.is_null();
            let _ = tx.send(ok);
        });
        unsafe {
            if store.respondsToSelector(sel!(requestFullAccessToEventsWithCompletion:)) {
                store.requestFullAccessToEventsWithCompletion(RcBlock::as_ptr(&block));
            } else {
                store.requestAccessToEntityType_completion(
                    EKEntityType::Event,
                    RcBlock::as_ptr(&block),
                );
            }
        }
        rx.recv().unwrap_or(false)
    }

    fn visible_calendars(store: &EKEventStore) -> Result<Vec<Retained<EKCalendar>>, SourceError> {
        let all = unsafe { store.calendarsForEntityType(EKEntityType::Event) };
        let mut kept = Vec::new();
        for calendar in all.iter() {
            if calendar_is_hidden(&calendar) {
                continue;
            }
            kept.push(calendar);
        }
        Ok(kept)
    }

    /// `isHidden` is the Calendar.app sidebar checkbox. objc2-event-kit 0.3.2 does not wrap it.
    fn calendar_is_hidden(calendar: &EKCalendar) -> bool {
        if !calendar.respondsToSelector(sel!(isHidden)) {
            return false;
        }
        let hidden: Bool = unsafe { msg_send![calendar, isHidden] };
        hidden.as_bool()
    }

    fn record_from_event(event: &EKEvent, zone: Tz) -> Option<EventRecord> {
        let event_identifier = unsafe { event.eventIdentifier() }
            .map(|value| value.to_string())
            .filter(|value| !value.is_empty())?;
        let calendar_item_identifier = unsafe { event.calendarItemIdentifier() }.to_string();
        if calendar_item_identifier.is_empty() {
            return None;
        }
        let calendar = unsafe {
            event
                .calendar()
                .map(|calendar| calendar.title().to_string())
                .unwrap_or_default()
        };
        let title = Some(unsafe { event.title() }.to_string());
        let location = unsafe { event.location() }.map(|value| value.to_string());
        let notes = unsafe { event.notes() }.map(|value| value.to_string());
        let url = unsafe { event.URL() }
            .and_then(|value| value.absoluteString())
            .map(|value| value.to_string());
        let start_date = unsafe { event.startDate() };
        let end_date = unsafe { event.endDate() };
        let start_utc = nsdate_to_utc(&start_date)?;
        let end_utc = nsdate_to_utc(&end_date)?;
        let all_day = unsafe { event.isAllDay() };
        let (start, end, displayed_start, exclusive_end, ek_start_utc) = if all_day {
            (
                None,
                None,
                Some(displayed_all_day_date(start_utc)),
                Some(end_utc.date_naive()),
                Some(start_utc),
            )
        } else {
            let (start, end) = timed_bounds(event, start_utc, end_utc, zone);
            (Some(start), Some(end), None, None, None)
        };
        let (attendees, user_status) = read_attendees(event);
        Some(EventRecord {
            event_identifier,
            calendar_item_identifier,
            calendar,
            title,
            all_day,
            start,
            end,
            displayed_start,
            exclusive_end,
            ek_start_utc,
            location,
            notes,
            url,
            event_status: event_word(unsafe { event.status() }).to_string(),
            user_status,
            attendees,
        })
    }

    fn timed_bounds(
        event: &EKEvent,
        start_utc: DateTime<Utc>,
        end_utc: DateTime<Utc>,
        zone: Tz,
    ) -> (DateTime<FixedOffset>, DateTime<FixedOffset>) {
        let event_zone = unsafe { event.timeZone() }
            .and_then(|time_zone| time_zone.name().to_string().parse::<Tz>().ok());
        if let Some(event_zone) = event_zone {
            (
                start_utc.with_timezone(&event_zone).fixed_offset(),
                end_utc.with_timezone(&event_zone).fixed_offset(),
            )
        } else {
            let offset = *start_utc.with_timezone(&zone).fixed_offset().offset();
            (
                start_utc.with_timezone(&offset),
                end_utc.with_timezone(&offset),
            )
        }
    }

    fn read_attendees(event: &EKEvent) -> (Vec<AttendeeRecord>, Option<String>) {
        let Some(people) = (unsafe { event.attendees() }) else {
            return (Vec::new(), None);
        };
        let mut attendees = Vec::new();
        let mut user_status = None;
        for person in people.iter() {
            let status = participant_word(unsafe { person.participantStatus() });
            if unsafe { person.isCurrentUser() } {
                user_status = Some(status.to_string());
            }
            let name = unsafe { person.name() }
                .map(|value| value.to_string())
                .unwrap_or_default();
            let person_url = unsafe { person.URL() };
            let email = participant_email(&person_url);
            attendees.push(AttendeeRecord {
                name,
                email,
                status: status.to_string(),
            });
        }
        (attendees, user_status)
    }

    fn participant_email(url: &NSURL) -> String {
        let Some(absolute) = url.absoluteString() else {
            return String::new();
        };
        let text = absolute.to_string();
        let Some((scheme, rest)) = text.split_once(':') else {
            return String::new();
        };
        if !scheme.eq_ignore_ascii_case("mailto") {
            return String::new();
        }
        percent_decode(rest)
    }

    fn percent_decode(value: &str) -> String {
        let bytes = value.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' && index + 2 < bytes.len() {
                if let (Some(hi), Some(lo)) =
                    (from_hex(bytes[index + 1]), from_hex(bytes[index + 2]))
                {
                    out.push((hi << 4) | lo);
                    index += 3;
                    continue;
                }
            }
            out.push(bytes[index]);
            index += 1;
        }
        String::from_utf8(out).unwrap_or_else(|_| value.to_string())
    }

    fn from_hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    fn event_word(status: EKEventStatus) -> &'static str {
        if status == EKEventStatus::Confirmed {
            "confirmed"
        } else if status == EKEventStatus::Tentative {
            "tentative"
        } else if status == EKEventStatus::Canceled {
            "canceled"
        } else {
            "none"
        }
    }

    fn participant_word(status: EKParticipantStatus) -> &'static str {
        if status == EKParticipantStatus::Accepted {
            "accepted"
        } else if status == EKParticipantStatus::Declined {
            "declined"
        } else if status == EKParticipantStatus::Tentative {
            "tentative"
        } else if status == EKParticipantStatus::Pending {
            "pending"
        } else if status == EKParticipantStatus::Delegated {
            "delegated"
        } else if status == EKParticipantStatus::Completed {
            "completed"
        } else if status == EKParticipantStatus::InProcess {
            "in_process"
        } else {
            "unknown"
        }
    }

    fn nsdate_to_utc(date: &NSDate) -> Option<DateTime<Utc>> {
        let secs = date.timeIntervalSince1970();
        if !secs.is_finite() {
            return None;
        }
        let whole = secs.trunc();
        let mut nanos = ((secs - whole) * 1_000_000_000.0).round() as i64;
        let mut seconds = whole as i64;
        if nanos >= 1_000_000_000 {
            seconds += 1;
            nanos -= 1_000_000_000;
        } else if nanos < 0 {
            seconds -= 1;
            nanos += 1_000_000_000;
        }
        DateTime::from_timestamp(seconds, nanos as u32)
    }

    fn unix_seconds(dt: DateTime<Utc>) -> f64 {
        dt.timestamp() as f64 + f64::from(dt.timestamp_subsec_nanos()) / 1_000_000_000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use chrono_tz::America::Los_Angeles;
    use serde::Deserialize;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn parse_dt(value: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(value).unwrap()
    }

    fn la_slice(day: NaiveDate) -> Slice {
        timeutil::whole_day_slice(day, Los_Angeles)
    }

    fn default_slice() -> Slice {
        la_slice(date(2026, 9, 27))
    }

    #[derive(Deserialize)]
    struct FixtureEvent {
        event_identifier: String,
        calendar_item_identifier: String,
        calendar: String,
        title: Option<String>,
        all_day: bool,
        start: Option<String>,
        end: Option<String>,
        displayed_start: Option<String>,
        exclusive_end: Option<String>,
        ek_start_utc: Option<String>,
        location: Option<String>,
        notes: Option<String>,
        url: Option<String>,
        event_status: String,
        user_status: Option<String>,
        #[serde(default)]
        attendees: Vec<FixtureAttendee>,
    }

    #[derive(Deserialize)]
    struct FixtureAttendee {
        #[serde(default)]
        name: String,
        #[serde(default)]
        email: String,
        #[serde(default)]
        status: String,
    }

    fn load_events(name: &str) -> Vec<EventRecord> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/calendar")
            .join(name);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let fixtures: Vec<FixtureEvent> = if value.is_array() {
            serde_json::from_value(value).unwrap()
        } else {
            vec![serde_json::from_value(value).unwrap()]
        };
        fixtures.into_iter().map(record_from_fixture).collect()
    }

    fn record_from_fixture(fixture: FixtureEvent) -> EventRecord {
        EventRecord {
            event_identifier: fixture.event_identifier,
            calendar_item_identifier: fixture.calendar_item_identifier,
            calendar: fixture.calendar,
            title: fixture.title,
            all_day: fixture.all_day,
            start: fixture.start.as_deref().map(parse_dt),
            end: fixture.end.as_deref().map(parse_dt),
            displayed_start: fixture
                .displayed_start
                .as_deref()
                .map(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").unwrap()),
            exclusive_end: fixture
                .exclusive_end
                .as_deref()
                .map(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").unwrap()),
            ek_start_utc: fixture
                .ek_start_utc
                .as_deref()
                .map(|value| parse_dt(value).with_timezone(&Utc)),
            location: fixture.location,
            notes: fixture.notes,
            url: fixture.url,
            event_status: fixture.event_status,
            user_status: fixture.user_status,
            attendees: fixture
                .attendees
                .into_iter()
                .map(|attendee| AttendeeRecord {
                    name: attendee.name,
                    email: attendee.email,
                    status: attendee.status,
                })
                .collect(),
        }
    }

    fn field<'a>(extra: &'a str, key: &str) -> &'a str {
        let prefix = format!("{key}: ");
        extra
            .lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .unwrap_or_else(|| panic!("missing {key} in {extra}"))
    }

    fn map_default(events: &[EventRecord], contacts: &ContactIndex) -> Vec<CollectedItem> {
        map_events(events, &default_slice(), Los_Angeles, contacts)
    }

    #[test]
    fn timed_event_files_on_start_instant() {
        let items = map_default(&load_events("timed.json"), &ContactIndex::empty());
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.t, parse_dt("2026-09-27T15:00:00-07:00"));
        assert_eq!(item.file_day, date(2026, 9, 27));
        assert_eq!(item.tie_break, "");
        assert_eq!(item.stable_id, "evt-dentist/2026-09-27T15:00:00-07:00");
        assert_eq!(field(&item.extra_front_matter, "all_day"), "false");
        assert_eq!(
            field(&item.extra_front_matter, "start"),
            "2026-09-27T15:00:00-07:00"
        );
        assert_eq!(
            field(&item.extra_front_matter, "end"),
            "2026-09-27T16:00:00-07:00"
        );
        assert_eq!(
            item.source_url,
            "ical://ekevent/20260927T220000Z/item%2Fdentist%2B1?method=show&options=more"
        );
        assert!(item.attachments.is_empty());
        assert_eq!(item.display_title, "Dentist");
        assert_eq!(item.empty_title_fallback, "untitled");
        assert_eq!(
            item.extra_front_matter,
            "\
calendar: \"Personal\"
status: accepted
all_day: false
start: 2026-09-27T15:00:00-07:00
end: 2026-09-27T16:00:00-07:00
location: \"123 Main St\"
attendees:
  - name: \"Ada Lovelace\"
    email: ada@example.com
    status: accepted
"
        );
    }

    #[test]
    fn timed_event_in_another_zone_files_locally() {
        let mut events = load_events("timed.json");
        events[0].start = Some(parse_dt("2026-09-27T18:00:00-04:00"));
        events[0].end = Some(parse_dt("2026-09-27T19:00:00-04:00"));
        let items = map_default(&events, &ContactIndex::empty());
        let item = &items[0];
        assert_eq!(item.t, parse_dt("2026-09-27T15:00:00-07:00"));
        assert_eq!(item.file_day, date(2026, 9, 27));
        assert_eq!(
            field(&item.extra_front_matter, "start"),
            "2026-09-27T18:00:00-04:00"
        );
        assert!(item.stable_id.ends_with("/2026-09-27T18:00:00-04:00"));
        assert!(item.source_url.contains("20260927T220000Z"));
    }

    #[test]
    fn all_day_uses_displayed_date() {
        let start = parse_dt("2026-09-27T00:00:00Z").with_timezone(&Utc);
        assert_eq!(displayed_all_day_date(start), date(2026, 9, 27));
        let items = map_default(&load_events("all_day.json"), &ContactIndex::empty());
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.t, parse_dt("2026-09-27T00:00:00-07:00"));
        assert_ne!(item.t, parse_dt("2026-09-26T17:00:00-07:00"));
        assert_eq!(item.file_day, date(2026, 9, 27));
        assert_eq!(item.stable_id, "evt-holiday/2026-09-27");
        assert_eq!(field(&item.extra_front_matter, "all_day"), "true");
        assert_eq!(field(&item.extra_front_matter, "start"), "2026-09-27");
        assert_eq!(field(&item.extra_front_matter, "end"), "2026-09-27");
        assert_eq!(
            item.source_url,
            "ical://ekevent/20260927T000000Z/item%2Fholiday%2B1?method=show&options=more"
        );
        assert!(item
            .extra_front_matter
            .lines()
            .all(|line| !line.starts_with("location:") && !line.starts_with("attendees:")));
        assert_eq!(field(&item.extra_front_matter, "status"), "accepted");
        assert_eq!(item.body, "");
    }

    #[test]
    fn all_day_span_is_one_item_on_the_start() {
        let event = EventRecord {
            event_identifier: "evt-span".to_string(),
            calendar_item_identifier: "item/span".to_string(),
            calendar: "Personal".to_string(),
            title: Some("Span".to_string()),
            all_day: true,
            start: None,
            end: None,
            displayed_start: Some(date(2026, 9, 27)),
            exclusive_end: Some(date(2026, 9, 30)),
            ek_start_utc: Some(parse_dt("2026-09-27T00:00:00Z").with_timezone(&Utc)),
            location: None,
            notes: None,
            url: None,
            event_status: "confirmed".to_string(),
            user_status: None,
            attendees: Vec::new(),
        };
        let items = map_default(std::slice::from_ref(&event), &ContactIndex::empty());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].file_day, date(2026, 9, 27));
        assert_eq!(field(&items[0].extra_front_matter, "end"), "2026-09-29");
        let later = map_events(
            std::slice::from_ref(&event),
            &la_slice(date(2026, 9, 28)),
            Los_Angeles,
            &ContactIndex::empty(),
        );
        assert!(later.is_empty());
    }

    #[test]
    fn multi_day_timed_is_one_item_on_start_day() {
        let events = load_events("multi_day_timed.json");
        let items = map_default(&events, &ContactIndex::empty());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].file_day, date(2026, 9, 27));
        assert_eq!(
            field(&items[0].extra_front_matter, "end"),
            "2026-09-29T06:00:00-07:00"
        );
        let later = map_events(
            &events,
            &la_slice(date(2026, 9, 28)),
            Los_Angeles,
            &ContactIndex::empty(),
        );
        assert!(later.is_empty());
    }

    #[test]
    fn recurring_keeps_occurrences_whose_t_is_in_the_slice() {
        let events = load_events("recurring.json");
        let items = map_default(&events, &ContactIndex::empty());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].stable_id, "evt-standup/2026-09-27T09:00:00-07:00");
        let wide = Slice {
            start: parse_dt("2026-09-20T00:00:00-07:00"),
            end: parse_dt("2026-10-05T00:00:00-07:00"),
            day: date(2026, 9, 20),
        };
        let all = map_events(&events, &wide, Los_Angeles, &ContactIndex::empty());
        assert_eq!(
            all.iter()
                .map(|item| item.stable_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "evt-standup/2026-09-20T09:00:00-07:00",
                "evt-standup/2026-09-27T09:00:00-07:00",
                "evt-standup/2026-10-04T09:00:00-07:00",
            ]
        );
    }

    #[test]
    fn declined_event_is_included() {
        let items = map_default(&load_events("declined.json"), &ContactIndex::empty());
        assert_eq!(items.len(), 1);
        assert_eq!(field(&items[0].extra_front_matter, "status"), "declined");
    }

    #[test]
    fn cancelled_event_is_included() {
        let items = map_default(&load_events("cancelled.json"), &ContactIndex::empty());
        assert_eq!(items.len(), 1);
        assert_eq!(field(&items[0].extra_front_matter, "status"), "cancelled");
    }

    #[test]
    fn missing_title_is_empty_display_title() {
        let items = map_default(&load_events("missing_title.json"), &ContactIndex::empty());
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].display_title, "");
        assert_eq!(items[0].empty_title_fallback, "untitled");
        assert!(items[0]
            .extra_front_matter
            .lines()
            .all(|line| !line.starts_with("location:") && !line.starts_with("attendees:")));
        assert_eq!(items[0].body, "");
    }

    #[test]
    fn attendees_resolve_sort_and_needs_action() {
        let vault = crate::testutil::TempVault::new();
        let dir = vault.root.join("contacts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Ada Lovelace.md"),
            "\
---
id: \"ada\"
emails:
  - value: ada@example.com
---
",
        )
        .unwrap();
        let index = ContactIndex::load(&dir);
        let items = map_default(&load_events("attendees.json"), &index);
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(field(&item.extra_front_matter, "status"), "needs_action");
        let ada = item.extra_front_matter.find("Ada Lovelace").unwrap();
        let grace = item.extra_front_matter.find("Grace Hopper").unwrap();
        assert!(ada < grace);
        assert!(item.extra_front_matter.contains("name: \"Ada Lovelace\""));
        assert!(item.extra_front_matter.contains("name: \"Grace Hopper\""));
        assert!(!item
            .extra_front_matter
            .lines()
            .any(|line| line.starts_with("location:")));
        assert_eq!(
            item.body,
            "\
Cleaning.

## Attendees

- [[Ada Lovelace]] <ada@example.com> (accepted)
- Grace Hopper <grace@example.com> (tentative)

URL: https://example.com/appointment
"
        );
        assert!(!item.body.starts_with('\n'));
        assert!(item.body.ends_with('\n'));
    }

    #[test]
    fn pending_attendee_word_is_needs_action() {
        let event = EventRecord {
            event_identifier: "evt-pending".to_string(),
            calendar_item_identifier: "item/pending".to_string(),
            calendar: "Personal".to_string(),
            title: Some("Pending".to_string()),
            all_day: false,
            start: Some(parse_dt("2026-09-27T11:00:00-07:00")),
            end: Some(parse_dt("2026-09-27T11:30:00-07:00")),
            displayed_start: None,
            exclusive_end: None,
            ek_start_utc: None,
            location: None,
            notes: None,
            url: None,
            event_status: "confirmed".to_string(),
            user_status: Some("pending".to_string()),
            attendees: vec![AttendeeRecord {
                name: "Pat".to_string(),
                email: "pat@example.com".to_string(),
                status: "pending".to_string(),
            }],
        };
        let items = map_default(std::slice::from_ref(&event), &ContactIndex::empty());
        assert_eq!(
            field(&items[0].extra_front_matter, "status"),
            "needs_action"
        );
        assert!(items[0].body.contains("(needs_action)"));
        assert!(items[0].extra_front_matter.contains("status: needs_action"));
    }

    #[test]
    fn omit_url_that_equals_source_url() {
        let mut events = load_events("timed.json");
        events[0].notes = Some("Cleaning.".to_string());
        events[0].attendees.clear();
        events[0].location = None;
        let first = &map_default(&events, &ContactIndex::empty())[0];
        events[0].url = Some(first.source_url.clone());
        let item = &map_default(&events, &ContactIndex::empty())[0];
        assert!(!item.body.contains("URL:"));
        assert_eq!(item.body, "Cleaning.\n");
    }

    #[test]
    fn predicate_pad_covers_all_day_utc_midnight() {
        let instant = parse_dt("2026-09-27T00:00:00Z").with_timezone(&Utc);
        let tokyo = Slice {
            start: parse_dt("2026-09-27T00:00:00+09:00"),
            end: parse_dt("2026-09-27T08:00:00+09:00"),
            day: date(2026, 9, 27),
        };
        let (start, end) = predicate_interval(&tokyo);
        assert!(start <= instant && instant < end);
        assert!(end - start < chrono::Duration::days(4 * 365));
        let (start, end) = predicate_interval(&default_slice());
        assert!(start <= instant && instant < end);
        assert!(end - start < chrono::Duration::days(4 * 365));
    }

    #[test]
    fn filter_drops_all_day_outside_the_slice() {
        let event = EventRecord {
            event_identifier: "evt-yesterday".to_string(),
            calendar_item_identifier: "item/yesterday".to_string(),
            calendar: "Personal".to_string(),
            title: Some("Yesterday".to_string()),
            all_day: true,
            start: None,
            end: None,
            displayed_start: Some(date(2026, 9, 26)),
            exclusive_end: Some(date(2026, 9, 27)),
            ek_start_utc: Some(parse_dt("2026-09-26T00:00:00Z").with_timezone(&Utc)),
            location: None,
            notes: None,
            url: None,
            event_status: "confirmed".to_string(),
            user_status: None,
            attendees: Vec::new(),
        };
        let items = map_default(std::slice::from_ref(&event), &ContactIndex::empty());
        assert!(items.is_empty());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn calendar_source_requires_macos() {
        let source = CalendarSource { zone: Los_Angeles };
        let err = source
            .fetch(&default_slice(), &ContactIndex::empty())
            .unwrap_err();
        assert_eq!(err.reason, "Calendar is only available on macOS");
    }
}
