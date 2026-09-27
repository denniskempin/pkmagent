# Calendar import

Shared vault, cursor, publishing, and file-writing rules are in [overview.md](overview.md). This file defines the macOS Calendar source.

## Fetch

Use the macOS Calendar store (EventKit), read-only. Do not call the Google Calendar REST API. Events appear only if that calendar is enabled in Calendar.app, including iCloud, Exchange, subscribed calendars, and Google accounts synced onto the Mac. Calendars unchecked in Calendar.app are skipped. An account that exists only on the web is out of scope.

Include every returned event: accepted, tentative, not yet responded, declined, and cancelled. Store the status in frontmatter. If the API does not return a cancelled occurrence, do not invent one.

- Timed events use the start instant for the window and are filed on the local civil date of that instant. A multi-day event is one file on the start day. The file records the real end.
- All-day events use the civil start date Calendar.app displays. Do not convert a UTC midnight through the local timezone. For the window only, treat the event as occurring at local midnight at the start of that displayed date. A run later the same day does not import that all-day event again. A multi-day all-day event is one file on its displayed start date. Store inclusive start and inclusive end dates. EventKit's exclusive end date must be converted to the inclusive last day the user sees.
- Recurring events are expanded. One file per occurrence whose window instant falls in the slice.

## Files

The stable id is the event identifier, a slash, and the occurrence start (`YYYY-MM-DD` for all-day, RFC3339 for timed). The frontmatter `id` is that value.

If any file in the day's `calendar/` directory has that occurrence id, do not modify it and do not create another file for that occurrence. Do not append to an event file. An event file that already exists is left unchanged even when the event later changes or is cancelled. If no file has that id, create one.

The display title for a new file is the event title, or `untitled`. Sanitize the title with the overview rules.

## Source link

```text
ical://ekevent/<utc>/<calendar item id>?method=show&options=more
```

`<utc>` is the occurrence start formatted in UTC as `yyyyMMdd'T'HHmmss'Z'`. An all-day event uses `00:00:00Z` on its displayed start date. `<calendar item id>` is the EventKit calendar item identifier, not the event identifier, and is percent-encoded. Calendar.app does not document this URL. Use it anyway: on current macOS it opens that event. The visible link text is `Open in Calendar`.

When the event also has its own URL, keep that URL on the `URL:` line in the body. Omit that line when it is missing or equal to `source_url`.

## File format

```yaml
---
source: calendar
id: "<event id>/<occurrence start>"
title: "Dentist"
source_url: "ical://ekevent/20260927T220000Z/ABC123?method=show&options=more"
day: YYYY-MM-DD
timezone: America/Los_Angeles
imported_at: 2026-09-27T18:04:11-07:00
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
---
```

`status` is one of `accepted`, `declined`, `tentative`, `cancelled`, `needs_action`, `unknown`. Use `needs_action` when the user has not responded. Omit `location` when empty. Omit `attendees` when there are none. Sort attendees by email, then name. An attendee's `name` is the contact title when the email matches a contact file, as defined in [contacts.md](contacts.md). Otherwise use the name Calendar provides.

Timed `start` and `end` are RFC3339 in the event's time zone. All-day `start` and `end` are inclusive civil dates (`YYYY-MM-DD`) as Calendar.app shows them. `all_day` is `true` or `false`.

```markdown
# Dentist

[Open in Calendar](ical://ekevent/20260927T220000Z/ABC123?method=show&options=more)

Cleaning.

## Attendees

- [[Ada Lovelace]] <ada@example.com> (accepted)

URL: https://example.com/appointment
```

Omit the description when the source has none. Omit the attendees section when there are none. A matched attendee uses the wiki link from [contacts.md](contacts.md). An unmatched attendee is plain text. The parenthesized word is that attendee's status.

## Edge cases

- An event whose start is at or after the frozen end is not imported by catch-up. A timed event later the same day is imported when a later run's window covers its start.
- A Wednesday–Friday event becomes one file in Wednesday's folder. The frontmatter end is Friday. A later run does not append to that file.
- An all-day event on March 1 stays in `inbox/2026-03-01/` even when the store encodes all-day events as UTC midnight.
- A changed or cancelled event is not removed or rewritten once its file exists. A later contacts import does not add wiki links to that file.
