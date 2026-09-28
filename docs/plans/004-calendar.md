# 004 — Calendar

Implementation of [../specs/calendar.md](../specs/calendar.md) against [001-foundation.md](001-foundation.md). `src/sources/calendar.rs` is `crate::sources::calendar`. It implements `CollectSource::fetch` and the pure mapping from a fixture event to `CollectedItem`. Foundation still owns the CLI, the lock, cursors, civil-date slicing, filename sanitizing, the collect write phase, progress lines, and the dated failure line.

`fetch` returns one `CollectedItem` per occurrence whose `t` falls in the slice. `tie_break` is empty. Each occurrence has its own `stable_id`, so foundation's per-directory grouping leaves them as separate files.

Live EventKit calls are `cfg(target_os = "macos")`. Mapping a fixture event is compiled and tested on Linux. No test constructs `EKEventStore`, and no test prompts TCC.

## Module

```text
src/sources/calendar.rs
tests/fixtures/calendar/
```

```rust
pub struct EventRecord { /* fixture or live snapshot; see Fixtures */ }
pub struct AttendeeRecord { /* name, email, status */ }

pub fn predicate_interval(slice: &Slice) -> (DateTime<Utc>, DateTime<Utc>)

pub fn displayed_all_day_date(start_utc: DateTime<Utc>) -> NaiveDate

pub fn map_events(
    events: &[EventRecord],
    slice: &Slice,
    zone: Tz,
    contacts: &ContactIndex,
) -> Vec<CollectedItem>

pub struct CalendarSource;

impl CollectSource for CalendarSource {
    fn fetch(
        &self,
        slice: &Slice,
        contacts: &ContactIndex,
    ) -> Result<Vec<CollectedItem>, SourceError>;
}
```

`SourceError` is the foundation source error. Its reason is the text foundation prints after `failed calendar: `. This module returns that error. It does not call `process::exit` and it does not format the dated line.

`map_events` drops an occurrence whose `t` is outside `[slice.start, slice.end)`. Foundation drops those again. `zone` is the machine IANA zone from `src/timeutil.rs`, the same zone the run uses for civil dates. Tests pass `America::Los_Angeles` or `Asia::Tokyo` from `chrono-tz`.

`collect` constructs `CalendarSource` on every target. On macOS, `fetch` runs the EventKit path below. On any other target, `fetch` returns `Calendar is only available on macOS` and does not open a store. Linux tests call `map_events` and `predicate_interval`, and they assert that non-macOS `fetch`.

## Dependencies

macOS target only. `eventsMatchingPredicate:` returns one `EKEvent` per occurrence in the predicate window, so recurring expansion is done with `objc2-event-kit`. The higher-level `eventkit` crate is the fallback only if that expansion cannot be done with objc2. It is unused here. Do not depend on it.

```toml
[target.'cfg(target_os = "macos")'.dependencies]
objc2-event-kit = "0.3.2"
```

Also depend directly on `objc2` (`>=0.6.2, <0.8.0`), `objc2-foundation` (`^0.3.2`), and `block2` (`>=0.6.1, <0.8.0`). Those are the ranges `objc2-event-kit` 0.3.2 declares, and the fetch code names those crates. Default features already include `block2`, `EKEventStore`, `EKEvent`, `EKCalendar`, `EKCalendarItem`, `EKParticipant`, `EKObject`, and `EKTypes`. Keep `block2` enabled so the completion handler stays available. `requestFullAccessToEventsWithCompletion:` is that `block2` handler. `chrono` and `chrono-tz` stay the shared foundation dependencies. Front matter stays hand-written. Do not add `serde_yaml`. Fixture JSON uses the shared `serde_json`.

Percent-encode the calendar item id in this module. Unreserved bytes are `A-Z a-z 0-9 - . _ ~`. Every other UTF-8 byte is `%` plus two uppercase hex digits. No extra crate.

The macOS binary embeds an `Info.plist` section so the full-access prompt has a reason string:

- `NSCalendarsFullAccessUsageDescription` — macOS 14 and later
- `NSCalendarsUsageDescription` — earlier macOS

Both values are `pkmagent reads your calendars to copy events into the inbox.` A `build.rs` passes `-Wl,-sectcreate,__TEXT,__info_plist,<path>` to the `pkmagent` bin on `cfg(target_os = "macos")` only.

## Permission

`fetch` creates one `EKEventStore` with `init` and uses it only on that call. Categories run concurrently with each other; this store is not shared, and this module does not change `NSTimeZone.defaultTimeZone`.

Read `EKEventStore::authorizationStatusForEntityType(EKEntityType::Event)` first.

| Status | Result |
| --- | --- |
| `FullAccess`, or `Authorized` on the pre-14 path | Query. |
| `Restricted` | `Err` reason `access restricted`. Do not query. |
| `Denied` | `Err` reason `access denied`. Do not query. |
| `NotDetermined`, `WriteOnly` | Request full access, then re-read. |

On macOS 14 and later the request is `requestFullAccessToEventsWithCompletion:`. Detect that selector at runtime with `respondsToSelector`. On earlier macOS, `WriteOnly` and `FullAccess` are absent; `NotDetermined` calls `requestAccessToEntityType:completion:` with `EKEntityType::Event`. Do not call `requestWriteOnlyAccessToEventsWithCompletion:` or the reminders request.

The completion handler only sends the granted flag across a channel. The fetch thread waits, then continues. A `false` grant or a non-null `NSError` is `access denied`. Do not put `localizedDescription` or `userInfo` in the reason. After a grant that was not already authorized, call `reset` before listing calendars, because a store created before access has an empty calendar list. Re-read the status. Continue only for `FullAccess` or `Authorized`. Any other status is `access denied`.

The reason has no trailing newline and no event title, notes, or attendee data. An empty calendar is `Ok(vec![])`.

## Calendars

Query `calendarsForEntityType(EKEntityType::Event)`. Pass that filtered array to the predicate. An empty filtered array returns `Ok(vec![])` and does not build a predicate. `nil` would search every calendar the store knows, including unchecked ones, so the call always passes the array.

Skip a calendar whose private `isHidden` selector returns true. `objc2-event-kit` 0.3.2 does not wrap that selector. Send it only when `respondsToSelector(isHidden)` is true:

```rust
let hidden: Bool = msg_send![&*calendar, isHidden];
```

A checked calendar stays, including a subscribed calendar (`isSubscribed` is not a skip). This module does not save, remove, or create events or calendars, and it does not call `commit`.

Assumption: on current macOS, `isHidden` is the Calendar.app sidebar checkbox. The live check, after the category exists, is an unchecked calendar absent from the run and a checked subscribed calendar present. If that check fails, stop and replace this selector. Do not guess a second private flag, and do not parse `Calendar.sqlitedb` in this version.

## Predicate and the filter on `t`

`eventsMatchingPredicate:` returns occurrences that overlap the predicate window, already expanded. Recurrence exceptions come back as the objects the store returns. This module does not read `EKRecurrenceRule` and does not fill in a date the array lacks. A cancelled occurrence is included only when that array contains it.

The header documents two constraints. The window is evaluated in `NSTimeZone.defaultTimeZone`, and a start-to-end span of four years or more is shortened to four years. A foundation slice is one civil date, so the padded window stays under that cap. Leave the default timezone alone: calendar fetch runs beside the other categories, and the default timezone is process-global.

All-day events are floating dates. EventKit stores the start as `00:00:00Z` on the displayed start date and the end as `00:00:00Z` on the day after the last displayed day. `t` for the window is local midnight of the displayed start, which is a different instant from that UTC midnight.

`predicate_interval` pads the slice by 14 hours on each side. Fourteen hours covers the civil offset range `UTC−12` through `UTC+14`.

```text
predicate_start = slice.start - 14 hours
predicate_end   = slice.end + 14 hours
```

Build both as `NSDate` absolute instants and pass them to `predicateForEventsWithStartDate:endDate:calendars:`. Then keep an occurrence only when `slice.start <= t < slice.end`.

The pad exists so the store still returns an all-day event whose `t` is inside the slice:

- East of UTC, local midnight is before `00:00:00Z`. A Tokyo slice `[2026-09-27T00:00:00+09:00, 2026-09-27T08:00:00+09:00)` contains `t` for the all-day event displayed 2026-09-27, and that `t` is `2026-09-26T15:00:00Z`. The event is stored as `[2026-09-27T00:00:00Z, 2026-09-28T00:00:00Z)`, which starts after the unpadded slice ends (`2026-09-26T23:00:00Z`). The end pad reaches `2026-09-27T13:00:00Z` and the stored start overlaps the predicate.
- West of UTC, the stored UTC midnight is before local midnight. An all-day event displayed 2026-09-27 is stored at `2026-09-27T00:00:00Z`. A Los Angeles midnight slice starts at `2026-09-27T07:00:00Z`. If the store requires the event start to fall inside the window, the unpadded start misses it. The start pad moves the predicate start to `2026-09-26T17:00:00Z`, which still covers `2026-09-27T00:00:00Z`.

The same pad also returns events that must be discarded. An all-day event displayed 2026-09-26 overlaps a padded Los Angeles slice for 2026-09-27. Its `t` is `2026-09-26T00:00:00-07:00`, which is before `slice.start`, so the filter drops it. A timed event that started before the slice and merely overlaps it is dropped the same way. A multi-day event, timed or all-day, is one occurrence on its start, so a later slice that only covers the tail does not keep it.

## Mapping

The live path copies each returned `EKEvent` into an `EventRecord` and calls `map_events`. `startDate` is the occurrence start, including a moved exception. `occurrenceDate` is the original recurrence slot and is not used for `t`, the stable id, or the source URL.

Skip an occurrence with an empty `eventIdentifier` or an empty `calendarItemIdentifier`. That skip does not fail the category. A store or predicate failure that cannot produce the array is `Err` reason `fetch failed`. An empty array is success with no items.

### Timed

`t` is the start instant with the offset of `zone` at that instant. `file_day` is the civil date of `t` in `zone`. Front matter `start` and `end` are RFC3339 in the event's `timeZone`, second resolution, numeric offset with a colon (`2026-09-27T15:00:00-07:00`). `to_rfc3339_opts(SecondsFormat::Secs, false)` produces that form. A nil event timezone is floating: format `start` and `end` with the same offset as `t`.

A multi-day timed event is one item. `file_day` is the local date of the start. `end` is the real end instant in the event's zone.

The stable id is the event identifier, a slash, and that same event-zone RFC3339 start. Example: event identifier `evt-dentist`, start `2026-09-27T15:00:00-07:00`:

```text
evt-dentist/2026-09-27T15:00:00-07:00
```

`tie_break` is `""`. An event scheduled in `America/New_York` at `2026-09-27T18:00:00-04:00`, with `zone` Los Angeles, has `t` `2026-09-27T15:00:00-07:00`, `file_day` `2026-09-27`, front matter start `2026-09-27T18:00:00-04:00`, and stable id `evt-dentist/2026-09-27T18:00:00-04:00`.

### All-day

`displayed_all_day_date` is the Gregorian year, month, and day of `startDate` in UTC. For a stored start `2026-09-27T00:00:00Z` and a Los Angeles zone, that date is `2026-09-27`. The local reading of the same instant is `2026-09-26T17:00:00-07:00`, and that reading is not the displayed date, not `t`, and not `file_day`.

`t` is `00:00:00` on the displayed start date, with the offset of `zone` at that local midnight. In Los Angeles on 2026-09-27, `t` is `2026-09-27T00:00:00-07:00`. `file_day` is the displayed start date. Midnight is unambiguous on a DST transition day; the fold is later, at 01:00.

The exclusive end is the UTC civil date of `endDate`. The inclusive end stored in front matter is that date minus one day. A one-day event whose exclusive end is `2026-09-28` stores `2026-09-27`. An event displayed 27–29 September has exclusive end `2026-09-30` and inclusive end `2026-09-29`. If the exclusive end is not after the displayed start, the inclusive end is the displayed start.

Front matter `start` and `end` are those inclusive `YYYY-MM-DD` dates. The stable id uses the displayed start. Example: event identifier `evt-holiday`, displayed start `2026-09-27`:

```text
evt-holiday/2026-09-27
```

`tie_break` is `""`. One all-day span is one item on the displayed start date.

### Source URL

```text
ical://ekevent/<utc>/<calendar item id>?method=show&options=more
```

`<utc>` is the occurrence start in UTC as `yyyyMMdd'T'HHmmss'Z'`. A timed start `2026-09-27T15:00:00-07:00` is `20260927T220000Z`. An all-day event uses `00:00:00Z` on the displayed start date, so 2026-09-27 is `20260927T000000Z`. That stamp is the displayed date, not the UTC conversion of local midnight.

`<calendar item id>` is `calendarItemIdentifier`, percent-encoded. The event identifier stays in the stable id only. The query string is the literal `?method=show&options=more`. The URL is undocumented. Use it.

Calendar item id `item/dentist+1`:

```text
ical://ekevent/20260927T220000Z/item%2Fdentist%2B1?method=show&options=more
ical://ekevent/20260927T000000Z/item%2Fholiday%2B1?method=show&options=more
```

The second line is the all-day form for calendar item id `item/holiday+1`.

### Titles and attachments

`display_title` is the event title. `None`, or a title whose trimmed text is empty, becomes `""`. `empty_title_fallback` is `untitled`, so foundation's sanitizer produces the `untitled` filename. `attachments` is empty. Calendar events in this source have no attachment list. Foundation still writes `[Open in Calendar](<source_url>)` and would append attachments if the vector were non-empty.

## Status

The fixture field `event_status` is EventKit's event status: `none`, `confirmed`, `tentative`, `canceled`. `user_status` is the current user's participant status, or null when `isCurrentUser` matches nobody. Live code sets `user_status` from that attendee's `participantStatus`. Participant words in fixtures are `accepted`, `declined`, `tentative`, `pending`, `unknown`, `delegated`, `completed`, `in_process`.

Front matter `status`:

| Condition | `status` |
| --- | --- |
| `event_status` is `canceled` | `cancelled` |
| else `user_status` is `accepted` | `accepted` |
| else `user_status` is `declined` | `declined` |
| else `user_status` is `tentative` | `tentative` |
| else `user_status` is `pending` | `needs_action` |
| else `user_status` is `unknown`, `delegated`, `completed`, or `in_process` | `unknown` |
| else `user_status` is null and `event_status` is `confirmed` | `accepted` |
| else `user_status` is null and `event_status` is `tentative` | `tentative` |
| else | `unknown` |

`needs_action` is the pending participant status: the user has not responded. A cancelled event stays `cancelled` when the user had already accepted. Declined and cancelled events that are in the array are mapped. A date the array does not contain is not added.

Attendee `status` uses the same participant map: `accepted`, `declined`, `tentative`, `pending` → `needs_action`, and the other participant words → `unknown`. The parenthesized body word is that attendee status. `cancelled` is the event status only.

Assumption: an event with no current-user attendee and `confirmed` is `accepted`. That is a calendar entry the user holds with no RSVP row. `pending` is the only input that becomes `needs_action`.

## Extra front matter

YAML, no document markers, this key order. Foundation inserts it after the shared keys and does not reorder it. The block ends with a newline.

```yaml
calendar: "Personal"
status: accepted
all_day: false
start: 2026-09-27T15:00:00-07:00
end: 2026-09-27T16:00:00-07:00
location: "123 Main St"
attendees:
  - name: "Ada Lovelace"
    email: ada@example.com
    status: accepted
```

`calendar`, `status`, `all_day`, `start`, and `end` are always present. `all_day` is `true` or `false`. `status` and attendee `status` are bare words. Timed `start` and `end` are the event-zone RFC3339 strings. All-day `start` and `end` are inclusive `YYYY-MM-DD`.

Double-quote `calendar`, `name`, and `location`. Escape `\`, `"`, and a raw newline as `\n`. Omit `location` when the trimmed location is empty. Omit `attendees` when the list is empty.

`email` is a plain scalar when it matches `^[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+$`. Otherwise double-quote it the same way. `ada@example.com` stays plain, matching the spec sample.

`name` is `ContactIndex::title_for_email` when that returns a title. Otherwise it is the name from Calendar, or `""` when Calendar has no name. The front matter value is a plain string. The wiki-link brackets belong in the body only. This module calls `title_for_email` and does not restate the trim or case rules.

Sort attendees by email bytes, then by the written `name` bytes. Empty email sorts first. Keep every attendee the store returned, including the current user and including declined attendees. Do not add the organizer a second time when they are absent from `attendees`.

Live email comes from the participant `URL`. A string whose scheme is `mailto`, compared case-insensitively, yields the rest of the string percent-decoded. Any other URL yields an empty email; the name is still listed.

## Body

`body` is the markdown after the source link. It does not contain the link, the title heading, or a leading blank line. Sections that are present are separated by one blank line. The string does not end with a newline.

1. The description is the event notes with surrounding whitespace trimmed. Omit it when that trim is empty.
2. `## Attendees`, a blank line, then one line per attendee, when the list is non-empty.
3. `URL: <event url>` when the event URL is set, trimmed non-empty, and different from `source_url`.

A matched attendee:

```markdown
- [[Ada Lovelace]] <ada@example.com> (accepted)
```

An unmatched attendee uses the calendar name, with no brackets:

```markdown
- Grace Hopper <grace@example.com> (tentative)
```

The parenthesized word is that attendee's mapped status. When the email is empty, omit the `<...>` segment. When the written name is empty, the line is `- <email> (status)`. When both are empty, omit that attendee.

A full body for the timed spec sample, whose event URL differs from the `ical://` source URL:

```markdown
Cleaning.

## Attendees

- [[Ada Lovelace]] <ada@example.com> (accepted)

URL: https://example.com/appointment
```

## Fixtures

JSON under `tests/fixtures/calendar/`. One object, or an array of objects for `recurring.json`. The loader fills an `EventRecord`. These files are the store. Tests never open EventKit.

```json
{
  "event_identifier": "evt-dentist",
  "calendar_item_identifier": "item/dentist+1",
  "calendar": "Personal",
  "title": "Dentist",
  "all_day": false,
  "start": "2026-09-27T15:00:00-07:00",
  "end": "2026-09-27T16:00:00-07:00",
  "displayed_start": null,
  "exclusive_end": null,
  "ek_start_utc": null,
  "location": "123 Main St",
  "notes": "Cleaning.",
  "url": "https://example.com/appointment",
  "event_status": "confirmed",
  "user_status": "accepted",
  "attendees": [
    { "name": "Ada Lovelace", "email": "ada@example.com", "status": "accepted" }
  ]
}
```

Timed events set `start` and `end` to event-zone RFC3339 and leave `displayed_start`, `exclusive_end`, and `ek_start_utc` null. All-day events set `displayed_start` and `exclusive_end` to `YYYY-MM-DD`, set `ek_start_utc` to the stored `00:00:00Z` instant, and leave `start` and `end` null. `user_status` null means the user is not an attendee. `title`, `location`, `notes`, and `url` may be null.

| File | What it locks |
| --- | --- |
| `tests/fixtures/calendar/timed.json` | The object above. |
| `tests/fixtures/calendar/all_day.json` | `all_day` true, `displayed_start` `2026-09-27`, `exclusive_end` `2026-09-28`, `ek_start_utc` `2026-09-27T00:00:00Z`, calendar item id `item/holiday+1`, event id `evt-holiday`, title `Holiday`, no location, no notes, no url, no attendees, `event_status` `confirmed`, `user_status` null. |
| `tests/fixtures/calendar/multi_day_timed.json` | Start `2026-09-27T22:00:00-07:00`, end `2026-09-29T06:00:00-07:00`, title `Trip`, no attendees. |
| `tests/fixtures/calendar/recurring.json` | Three timed occurrences, same event id `evt-standup`, same calendar item id `item/standup`, starts `2026-09-20T09:00:00-07:00`, `2026-09-27T09:00:00-07:00`, and `2026-10-04T09:00:00-07:00`. Each ends 15 minutes later. |
| `tests/fixtures/calendar/declined.json` | `event_status` `confirmed`, `user_status` `declined`, title `Declined lunch`. |
| `tests/fixtures/calendar/cancelled.json` | `event_status` `canceled`, `user_status` `accepted`, title `Cancelled sync`. One object. The missing next occurrence is not in the file. |
| `tests/fixtures/calendar/missing_title.json` | `title` null, `notes` null, `location` null, `url` null, `attendees` empty, timed start `2026-09-27T11:00:00-07:00`. |
| `tests/fixtures/calendar/attendees.json` | `user_status` `pending`. Attendees in this order: `Grace Hopper` / `grace@example.com` / `tentative`, then `Ada Lovelace` / `ada@example.com` / `accepted`. Notes `Cleaning.` Location null. URL `https://example.com/appointment`. |

The attendee test writes a temp contact file `Ada Lovelace.md` and loads it with the `ContactIndex` constructor collect uses:

```markdown
---
id: "ada"
emails:
  - value: ada@example.com
---
```

`title_for_email("ada@example.com")` returns `Ada Lovelace`. `title_for_email("grace@example.com")` returns none. The test does not reimplement matching.

Default slice in these tests: `America::Los_Angeles`, `start` `2026-09-27T00:00:00-07:00`, `end` `2026-09-28T00:00:00-07:00`, `day` `2026-09-27`. On that date the zone offset is `-07:00`.

## Tests

`#[cfg(test)]` in `src/sources/calendar.rs`. `CARGO_MANIFEST_DIR` locates fixtures. Nothing in this list calls EventKit, Contacts, or `chat.db`. `cargo test` on Linux runs all of them.

- `timed_event_files_on_start_instant`. `timed.json` yields one item. `t` is `2026-09-27T15:00:00-07:00`, `file_day` is `2026-09-27`, `tie_break` is empty, `stable_id` is `evt-dentist/2026-09-27T15:00:00-07:00`, `all_day` is false, and `start` / `end` are `2026-09-27T15:00:00-07:00` / `2026-09-27T16:00:00-07:00`. `source_url` is `ical://ekevent/20260927T220000Z/item%2Fdentist%2B1?method=show&options=more`. `attachments` is empty. `display_title` is `Dentist`. `empty_title_fallback` is `untitled`.
- `timed_event_in_another_zone_files_locally`. The timed record with `start` `2026-09-27T18:00:00-04:00` and `end` `2026-09-27T19:00:00-04:00`, mapped in `America::Los_Angeles`, has local `t` `2026-09-27T15:00:00-07:00`, `file_day` `2026-09-27`, front matter start left in `-04:00`, and stable id ending in `/2026-09-27T18:00:00-04:00`. The UTC stamp in `source_url` is `20260927T220000Z`.
- `all_day_uses_displayed_date`. `displayed_all_day_date` of `2026-09-27T00:00:00Z` is `2026-09-27`. `all_day.json` yields `t` `2026-09-27T00:00:00-07:00`, `file_day` `2026-09-27`, stable id `evt-holiday/2026-09-27`, `all_day: true`, `start: 2026-09-27`, `end: 2026-09-27`, and `source_url` `ical://ekevent/20260927T000000Z/item%2Fholiday%2B1?method=show&options=more`. The item's `t` is not `2026-09-26T17:00:00-07:00`. `location` and `attendees` are absent. `status` is `accepted` because `user_status` is null and the event is `confirmed`. The body is empty.
- `all_day_span_is_one_item_on_the_start`. An all-day record with `displayed_start` `2026-09-27` and `exclusive_end` `2026-09-30` stores inclusive `end: 2026-09-29`, `file_day` `2026-09-27`, and one item. The same record mapped with a slice of `2026-09-28` returns no items.
- `multi_day_timed_is_one_item_on_start_day`. `multi_day_timed.json` on the 27 September slice yields one item, `file_day` `2026-09-27`, `end` `2026-09-29T06:00:00-07:00`. The slice for `2026-09-28` returns no items.
- `recurring_keeps_occurrences_whose_t_is_in_the_slice`. `recurring.json` on the 27 September slice yields one item, stable id `evt-standup/2026-09-27T09:00:00-07:00`. A slice from `2026-09-20T00:00:00-07:00` through `2026-10-05T00:00:00-07:00` yields three items and three stable ids. The mapper does not invent a fourth date.
- `declined_event_is_included`. `declined.json` yields one item with `status: declined`.
- `cancelled_event_is_included`. `cancelled.json` yields one item with `status: cancelled` even though `user_status` is `accepted`. The result has one item. No second occurrence is created for a date absent from the file.
- `missing_title_is_empty_display_title`. `missing_title.json` has `display_title` `""` and `empty_title_fallback` `untitled`. `location` and `attendees` are absent. The body is empty.
- `attendees_resolve_sort_and_needs_action`. `attendees.json` with the Ada index: front matter `status` is `needs_action`. Attendees are Ada then Grace. Ada's `name` is `Ada Lovelace`. Grace's `name` is `Grace Hopper`. The body is the description, then `## Attendees`, then `- [[Ada Lovelace]] <ada@example.com> (accepted)`, then `- Grace Hopper <grace@example.com> (tentative)`, then `URL: https://example.com/appointment`. `location` is absent.
- `pending_attendee_word_is_needs_action`. An attendee `status` of `pending` renders `(needs_action)` and front matter `status: needs_action`.
- `omit_url_that_equals_source_url`. A record whose `url` equals the computed `ical://` URL has no `URL:` line.
- `predicate_pad_covers_all_day_utc_midnight`. For the Tokyo slice `[2026-09-27T00:00:00+09:00, 2026-09-27T08:00:00+09:00)`, `predicate_interval` covers `2026-09-27T00:00:00Z`. For the Los Angeles midnight slice of 2026-09-27, it covers `2026-09-27T00:00:00Z` as well. Both spans are under four years.
- `filter_drops_all_day_outside_the_slice`. The all-day record for displayed `2026-09-26`, mapped against the 27 September Los Angeles slice, returns no items.
- `calendar_source_requires_macos`, under `cfg(not(target_os = "macos"))`. `CalendarSource::fetch` returns `Calendar is only available on macOS`.

## Runtime

The live check is foundation's collect run, against a release binary at a stable path. Calendar TCC is granted to that path. `cargo run` changes the binary identity, so the grant does not follow it. A denied or write-only grant exits `2`, writes no run folder, leaves cursors unchanged, and prints `YYYY-MM-DD failed calendar: access denied` on stderr. Foundation prints that line.

## Out of scope

- The Google Calendar REST API.
- Reminders, and any use of `EKEntityTypeReminder`.
- Writing, deleting, or editing events.
- Attachment bytes.
