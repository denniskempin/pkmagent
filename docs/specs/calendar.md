# Calendar

See [overview.md](overview.md). Collected by `pkmagent collect`.

## Fetch

Use EventKit, read-only. Include events on calendars enabled in Calendar.app. Skip calendars that are unchecked.

Include accepted, tentative, not-yet-responded, declined, and cancelled events that EventKit returns. Do not invent a cancelled occurrence the store does not return.

- Timed events use the start instant as `t` and are filed on that local civil date. A multi-day event is one file on the start day. The file records the real end.
- All-day events use the civil dates Calendar.app displays. Do not shift a UTC midnight into the local zone. For the window only, `t` is local midnight at the start of the displayed start date. Store inclusive dates. Convert EventKit's exclusive end to the last day the user sees.
- Expand recurring events. One file per occurrence whose `t` falls in the slice.

## Files

The stable id is the event identifier, a slash, and the occurrence start (`YYYY-MM-DD` for all-day, RFC3339 for timed).

The display title is the event title, or `untitled`.

## Source link

```text
ical://ekevent/<utc>/<calendar item id>?method=show&options=more
```

`<utc>` is the occurrence start in UTC as `yyyyMMdd'T'HHmmss'Z'`. An all-day event uses `00:00:00Z` on its displayed start date. `<calendar item id>` is the EventKit calendar item identifier, not the event identifier, percent-encoded. The URL is undocumented. Use it: on current macOS it opens that event.

## Extra front matter

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

`status` is `accepted`, `declined`, `tentative`, `cancelled`, `needs_action`, or `unknown`. Use `needs_action` when the user has not responded. Sort attendees by email, then name. `name` is the contact title when the email matches, otherwise the name from Calendar.

Timed `start` and `end` are RFC3339 in the event's zone. All-day `start` and `end` are inclusive `YYYY-MM-DD` dates as Calendar.app shows them.

## Body

After the source link: the description, then attendees, then the event's own URL when it is set and differs from `source_url`.

```markdown
Cleaning.

## Attendees

- [[Ada Lovelace]] <ada@example.com> (accepted)

URL: https://example.com/appointment
```

Omit the description, the attendees section, or the `URL` line when there is nothing to put there. The parenthesized word is that attendee's status.
