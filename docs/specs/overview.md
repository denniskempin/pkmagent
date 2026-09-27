# pkmagent

`pkmagent` is a local read-only importer. It pulls Gmail, macOS Messages, and macOS Calendar into a Markdown inbox that a personal management system then handles.

This document is the behavior spec. It does not choose a programming language, library, or Google Cloud project setup beyond the files and rules below.

## Workflow

The inbox is a drop zone, not an archive.

1. `pkmagent import` writes one folder per day into the inbox.
2. The personal management system handles those Markdown files.
3. The user removes everything inside the inbox. After that, the inbox is empty.
4. The next import writes a new set of day folders.

The tool does not empty the inbox at the end of a run. That would delete the files just imported. The tool also does not remember previous imports inside the inbox. Catch-up state and the Gmail credential live outside the inbox, so emptying the inbox does not import the same items again.

The cursor is a timestamp, not a date. A run at 10:00 records 10:00. A run at 18:00 imports messages, mail, and events from 10:00 onward, including the rest of that same day, and does not import the morning again.

## Vault layout

The inbox is a directory. The default is `./inbox`, resolved to an absolute path at startup from the current working directory. `--inbox PATH` replaces the default. The tool creates the inbox directory if it is missing.

A published day looks like this:

```text
inbox/
  2026-09-27/
    gmail/
      Quarterly plan.md
    messages/
      Ada Lovelace.md
    calendar/
      Dentist.md
```

- `YYYY-MM-DD` is the calendar day the item happened, in the machine's local timezone at the moment of that import. It is not the day the import started.
- Source directory names are exactly `gmail`, `messages`, and `calendar`.
- A source directory is created only when it contains at least one file.
- A day directory is created only when at least one source has a file.
- Files are UTF-8 Markdown with LF line endings and a trailing newline.
- The tool writes only under the inbox, and only by publishing whole day directories. It also deletes leftover `inbox/.tmp-*` directories, as defined below.
- The tool does not modify files outside a day directory it is publishing. A non-day file the user placed in the inbox stays in place.

Credentials, tokens, and the catch-up cursor are never written into the inbox.

## State outside the inbox

All of these paths are under the home directory.

| Path | Purpose |
| --- | --- |
| `~/.config/pkmagent/gmail-client.json` | OAuth client id and secret supplied by the user. Keys: `client_id`, `client_secret`. |
| `~/.config/pkmagent/gmail.json` | Refresh token for the single authorized Gmail account. Mode `0600`. |
| `~/.config/pkmagent/state.json` | Catch-up cursor. |
| `~/.config/pkmagent/import.lock` | Exclusive lock while an import is running. |

`state.json` contains one field:

```json
{"last_success_at": "2026-09-27T18:04:11-07:00"}
```

`last_success_at` is an RFC3339 timestamp with a numeric offset. It is the exclusive end of the last successful catch-up: every item at or after this instant is still eligible, and every item before it is not imported again. Missing file, or a file whose only value is `null`, means there has never been a successful catch-up. A file that exists but is not valid JSON of that shape is an error: the tool writes nothing and exits `1`.

The tool never prints tokens, client secrets, or message bodies in logs.

## Commands

### `pkmagent auth gmail`

Authorizes exactly one Gmail account with OAuth for the scope `https://www.googleapis.com/auth/gmail.readonly`. Stores the credential in `gmail.json`, replacing any previous account. Does not read or write the inbox and does not change the cursor.

The Gmail client file must already exist. If it is missing, the command exits `1`.

### `pkmagent import`

Catches up from the cursor through the moment the run starts.

- No cursor: `--since YYYY-MM-DD` is required. The window starts at local midnight at the beginning of that date.
- Cursor present: `--since` is rejected. The window starts at `last_success_at`.
- The window ends at the current time, frozen when the run starts. Items that arrive while the run is in progress wait for the next run.
- `--since` whose midnight is after that frozen end is rejected.
- `--day` together with `--since` is rejected.

An item is included when its instant `t` satisfies `window_start <= t < window_end`. On success the cursor becomes `window_end`. Days are published from oldest to newest. A later day is not published in this run if an earlier day failed.

### `pkmagent import --day YYYY-MM-DD`

Replaces that one civil day and does not change the cursor, whether the date is in the past, today, or the future. The slice is the full local day, from midnight inclusive to the next midnight exclusive. `--since` is not used. This can import items the cursor has already passed.

### Shared rules

- `--inbox PATH` is optional on import commands.
- Invalid arguments, a corrupt state file, a missing inbox parent that cannot be created, or an existing lock: write no day directories, leave the cursor unchanged, exit `1`.
- One import at a time. At start, if `import.lock` exists, exit `1` and tell the user to delete the lock if no import is running. Remove the lock when the process exits. A crash can leave the lock behind.
- Overlapping imports are not supported. There is no `--force`.
- If the frozen end is earlier than `last_success_at`, exit `1` and write nothing. If they are equal, the window is empty: exit `0`, write no day directories, and leave the cursor unchanged.

There is no `--from`, `--to`, or `--only`. Every import publishes Gmail, Messages, and Calendar together for each day.

## Publishing a day

A day is all or nothing across the three sources. The tool fetches the whole slice before it changes that day's folder.

The catch-up visits every local civil date from the date of `window_start` through the date of the last instant inside the window. Iterate civil dates, not 24-hour blocks. A DST transition must not skip or repeat a calendar date. An empty window visits no dates.

For civil date `D`, the slice is the intersection of the window with that date:

- `slice_start` is the later of `window_start` and local midnight at the start of `D`.
- `slice_end` is the earlier of `window_end` and local midnight at the start of `D+1`.
- Include items with `slice_start <= t < slice_end`.

A catch-up that starts at 10:00 and runs again at 18:00 visits only today. The slice is 10:00 inclusive through 18:00 exclusive. Mail from 11:00 is included. Mail from 09:00 is not.

1. At startup, delete every directory in the inbox whose name starts with `.tmp-`.
2. Fetch Gmail, Messages, and Calendar for the slice.
3. If any fetch fails, do not change `inbox/D`. Stop the run. The cursor stays where the previous successful slice left it.
4. If the slice has no items, do not create or delete `inbox/D`. Count the slice as published.
5. Otherwise render the files into `inbox/.tmp-D-<random>/`, delete `inbox/D` if it exists, and rename the temp directory to `inbox/D`. The new folder contains only this slice. It is not merged with files already in the folder.
6. On catch-up, after the slice is published, set `last_success_at` to `slice_end`. If that is still before `window_end`, continue to the next date. If it equals `window_end`, the run is finished.
7. `--day` uses steps 2 through 5 but does not change the cursor. A `--day` slice with no items deletes `inbox/D` if it exists.

A crash after the old directory is deleted and before the rename finishes leaves that day missing. The cursor is not advanced, so the next run publishes the slice again. A leftover temp directory is removed on the next startup.

The first day of a catch-up failing leaves a missing cursor missing, and leaves an existing cursor unchanged. A failure on a later day keeps the cursor at that day's `slice_start`, which is local midnight when the previous day was fully published.

## Sources

The tool never sends mail, never edits Messages, and never edits Calendar.

### Gmail

Use the Gmail API with the stored readonly credential. There is one account, the account that `auth gmail` stored.

Include every message whose Gmail `internalDate` falls in the slice, whether it was received or sent, including archived mail. Ignore the `Date` header. Exclude Spam, Trash, and Drafts. The instant used for the window is `internalDate`.

Group the slice's messages by Gmail thread id. The file contains only messages from that thread in the slice, oldest `internalDate` first. Ties break by Gmail message id, ascending. Deduplicate by message id. Quoted history inside a body stays. The tool does not pull messages outside the slice into the file, including earlier messages from the same day.

A thread can produce a separate file on each day it has messages. Sent and received messages in the same thread and the same slice are one file. A later slice the same day produces a new file with only the new messages.

If Gmail auth is missing or expired, that day's Gmail fetch fails and the day is not published.

### Messages

Read the local Messages store at `~/Library/Messages/chat.db` read-only. This includes iMessage, SMS, and RCS stored there. The tool needs Full Disk Access. A locked database or a permission failure fails that day's Messages fetch.

Include every chat, including group chats and messages the user sent. Include plain text, attachment-only messages, tapback reactions, and stickers as their own timestamped entries. Omit unsent messages.

The day of a message is its sent timestamp in the local timezone, not the read timestamp. That sent timestamp is the instant used for the window.

Group by chat. The file contains only that chat's messages in the slice, oldest first. Ties break by the database message id, ascending.

### Calendar

Use the macOS Calendar store (EventKit), read-only. Do not call the Google Calendar REST API. Events appear only if that calendar is enabled in Calendar.app, including iCloud, Exchange, subscribed calendars, and Google accounts synced onto the Mac. Calendars unchecked in Calendar.app are skipped. An account that exists only on the web is out of scope.

Include every returned event: accepted, tentative, not yet responded, declined, and cancelled. Store the status in frontmatter. If the API does not return a cancelled occurrence, do not invent one.

- Timed events use the start instant for the window and are filed on the local civil date of that instant. A multi-day event is one file on the start day. The file records the real end.
- All-day events use the civil start date Calendar.app displays. Do not convert a UTC midnight through the local timezone. For the window only, treat the event as occurring at local midnight at the start of that displayed date. A run later the same day does not import that all-day event again. A multi-day all-day event is one file on its displayed start date. Store inclusive start and inclusive end dates. EventKit's exclusive end date must be converted to the inclusive last day the user sees.
- Recurring events are expanded. One file per occurrence whose window instant falls in the slice.

## Filenames

Compute the display title, then sanitize it once while rendering the day. There is no later rename pass.

| Source | Display title |
| --- | --- |
| Gmail | Subject of the chronologically last included message, including `Re:` and `Fwd:`. Empty subject becomes `(no subject)`. |
| Direct message chat | The other participant's display name if Messages has one, otherwise the phone number or email exactly as stored. A chat with only the user is `Me`. |
| Group chat | The group display name if it has one. Otherwise the participant labels sorted by raw UTF-8 bytes, joined with `, `. |
| Calendar | The event title, or `untitled`. |

The other participant in a direct chat is the participant that is not the user's own Messages account. Phone numbers and emails are not reformatted. A whitespace-only group name counts as no name. Contact names are whatever Messages associates with the handle at import time.

Sanitizing a title:

1. Collapse all whitespace, including newlines, to a single space and trim the ends.
2. If the result is empty, use the empty-title fallback from the table above.
3. Replace `\ / : * ? " < > |` and ASCII control characters with `-`.
4. Trim trailing spaces and dots.
5. If the result is empty, `.`, or `..`, use `untitled`.
6. Truncate to 80 Unicode scalar values.
7. After the collision suffix is applied, if the UTF-8 filename including `.md` would exceed 200 bytes, truncate the title further until it fits.

Collision groups are titles that match after Unicode case fold, inside the same day and the same source. A group of one is `Title.md`. A group of more than one names every member `Title--<suffix>.md`, keeping each file's own casing. The suffix is the stable id sanitized with the same character rules, truncated to 40 Unicode scalar values. If two suffixes in the group still match, append `-2`, `-3`, and so on, ordered by the raw stable id ascending.

Stable ids:

- Gmail: the thread id.
- Messages: the chat guid.
- Calendar: the event identifier, a slash, and the occurrence start (`YYYY-MM-DD` for all-day, RFC3339 for timed).

## Markdown files

Every file starts with YAML frontmatter. Rendering is deterministic except for `imported_at`, which is the RFC3339 time the day was rendered.

### Gmail

```yaml
---
source: gmail
id: "<thread id>"
title: "<single-line subject>"
day: YYYY-MM-DD
timezone: America/Los_Angeles
imported_at: 2026-09-27T18:04:11-07:00
account: name@gmail.com
participants:
  - name: "Ada Lovelace"
    email: ada@example.com
---
```

`timezone` is the IANA name of the machine's local zone. `participants` is the union of From, To, Cc, and Bcc on the included messages, unique by email address, sorted by email. Use an empty name when the display name is unknown.

Use `text/plain` when that part exists. Otherwise convert HTML to plain text: decode entities, drop `script` and `style`, keep quoted text, and render a link as `label (url)` when the label differs from the url. Do not download attachments.

```markdown
# Quarterly plan

## 2026-09-26 08:14:03 -0700 — Ada Lovelace <ada@example.com>

- Message-Id: <id@mail.gmail.com>
- To: you@example.com
- Cc: team@example.com

Body of the message.

### Attachments

- plan.pdf (application/pdf)
```

Omit `Cc` and `Bcc` when empty. Omit the attachments section when there are none. An attachment with no filename is `unnamed`. List the media type when the source provides it. Order messages oldest first.

### Messages

```yaml
---
source: messages
id: "<chat guid>"
title: "Ada Lovelace"
day: YYYY-MM-DD
timezone: America/Los_Angeles
imported_at: 2026-09-27T18:04:11-07:00
participants:
  - name: "Ada Lovelace"
    handle: "+15551212"
  - name: ""
    handle: "me@icloud.com"
    self: true
---
```

Participants are sorted by handle, raw UTF-8 order. The user's own account has `self: true`. Outgoing sender label is `Me`.

```markdown
# Ada Lovelace

## 2026-09-27 09:01:00 -0700 — Ada Lovelace

See you there.

### Attachments

- photo.jpg (image/jpeg)
```

A reaction entry's body is one line:

```text
Reaction: Like to "dinner at 7"
```

The quoted target is the reacted-to message's plain text, whitespace collapsed, truncated to 80 scalar values. If it has no text, use its first attachment filename. If neither exists, use `a message`. Use the database's reaction name when it has one (`Love`, `Like`, `Dislike`, `Laugh`, `Emphasize`, `Question`, or a custom value).

A sticker entry's body is `Sticker: <filename>` or `Sticker: sticker` when there is no filename.

### Calendar

```yaml
---
source: calendar
id: "<event id>/<occurrence start>"
title: "Dentist"
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

`status` is one of `accepted`, `declined`, `tentative`, `cancelled`, `needs_action`, `unknown`. Use `needs_action` when the user has not responded. Omit `location` when empty. Omit `attendees` when there are none. Sort attendees by email, then name.

Timed `start` and `end` are RFC3339 in the event's time zone. All-day `start` and `end` are inclusive civil dates (`YYYY-MM-DD`) as Calendar.app shows them. `all_day` is `true` or `false`.

```markdown
# Dentist

Cleaning.

URL: https://example.com/appointment
```

Omit the description or the URL line when the source has none.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Every requested day was published. A day with zero items is success. |
| `1` | Invalid arguments, corrupt state, missing OAuth client on `auth`, clock went backward, or the lock exists. No day directory was changed. |
| `2` | A requested source failed. Earlier days in a catch-up window may already have been published. The failed day is unchanged. |

Stdout gets one line per published day: `YYYY-MM-DD gmail=<files> messages=<files> calendar=<files>`. Stderr gets `YYYY-MM-DD failed <source>: <reason>` for a failed day. Counts are files, not individual messages.

## Edge cases

- The inbox is expected to be empty before the next import because the user removed its contents. The tool does not delete day directories outside the slices it is publishing. A catch-up slice with no new items leaves an existing day directory in place.
- Catch-up does not patch files or merge by id. A slice that has items replaces that day folder with those items only. Morning files still in the folder are removed. `--day` replaces the folder with the full civil day.
- Two threads titled `Hello` on the same day both receive a stable-id suffix. One thread titled `Hello` is `Hello.md`.
- A subject or contact rename is visible only after the day is published again. The new folder contains only the new filename.
- An unsent message, a message moved to Trash, or an event that moved off that day is absent from the new folder. Items that disappeared are not deleted from some other day unless that other day is published too.
- A message or event before `last_success_at` is not imported. Publish that civil day explicitly with `--day` when the missed item should be fetched anyway.
- An event whose start is at or after the frozen end is not imported by catch-up. A timed event later the same day is imported when a later run's window covers its start. `--day` imports every occurrence on that date regardless of the cursor.
- Changing the machine timezone can change which civil date an instant falls on. The next publish of the affected days uses the timezone in effect at that run. All-day events still use the date Calendar.app displays.
- Mail that arrived during the day and was later archived is included. Spam, Trash, and Drafts are not.
- An `internalDate` is included only when it falls inside the slice. A timestamp after the frozen end waits for the next run.
- The same Gmail thread on Monday and Tuesday becomes two files. Each file has only that day's messages.
- A Wednesday–Friday event becomes one file in Wednesday's folder. The frontmatter end is Friday.
- An all-day event on March 1 stays in `inbox/2026-03-01/` even when the store encodes all-day events as UTC midnight.
- Gmail failure while Messages and Calendar succeeded does not publish the day. The previous folder, if any, stays as it was.
- A second catch-up the same day, with no items at or after the cursor, writes no day directories and advances the cursor to the new frozen end.
- A message at 15:00 is absent from a 10:00 run and present in an 18:00 run. A message at 09:00 is present only in the run whose window covered 09:00.
- `--day` on a date the user has not emptied replaces that day in place and leaves the catch-up cursor where it is.

## Worked example

The machine timezone is `America/Los_Angeles`. There is no cursor. The user runs:

```text
pkmagent import --since 2026-09-26
```

at 10:00 on 2026-09-27.

The window is `2026-09-26T00:00:00-07:00` inclusive through `2026-09-27T10:00:00-07:00` exclusive. The run publishes `2026-09-26` as the full civil day, then `2026-09-27` with only items before 10:00. A thread with mail on both days produces two Gmail files. A message at 11:00 is not in this run. A dentist appointment that runs from 2026-09-30 through 2026-10-02 is not in this run. After success, `last_success_at` is `2026-09-27T10:00:00-07:00`.

The user files the Markdown into the personal management system and deletes both day directories. The inbox is empty. The cursor remains.

At 18:00 the same day the user runs `pkmagent import`. The window is `2026-09-27T10:00:00-07:00` inclusive through `2026-09-27T18:00:00-07:00` exclusive. Today's folder is created with only those items, including the 11:00 message and excluding the 09:00 message. The cursor becomes `2026-09-27T18:00:00-07:00`.

## Out of scope

- Any source other than Gmail, the local Messages store, and the local Calendar store.
- Writing back to Gmail, Messages, or Calendar.
- Downloading attachment bytes.
- More than one Gmail account.
- Calling the Google Calendar REST API.
- A per-file history, a sync database inside the inbox, or merging into files the user edited.
- Emptying the inbox after a successful import.
- Choosing an implementation language.
