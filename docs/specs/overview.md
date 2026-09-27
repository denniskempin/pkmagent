# pkmagent

`pkmagent` is a local read-only importer. It pulls Gmail, macOS Messages, and macOS Calendar into a Markdown inbox that a personal management system then handles.

This document is the behavior spec. It does not choose a programming language, library, or Google Cloud project setup beyond the files and rules below.

## Workflow

The inbox is a drop zone, not an archive. Files from an earlier run may still be there.

1. `pkmagent import` creates Markdown files under one folder per day, or appends to a leftover file from an earlier run.
2. The personal management system handles those files. It may leave some of them in the inbox.
3. The next import appends new messages to a leftover file with the same id. It creates a file when no leftover has that id.
4. When the inbox has been emptied, the next import creates new files for items after the cursor.

The tool does not empty the inbox at the end of a run. Catch-up state lives in the vault’s `.pkmagent` directory, and the Gmail credential lives in `.pkmagent/secrets`. Both are outside the inbox, so emptying the inbox does not import the same items again.

The cursor is a timestamp, not a date. A run at 10:00 records 10:00. A run at 18:00 imports messages, mail, and events from 10:00 onward, including the rest of that same day. Morning items are not fetched again. If the morning file is still in the inbox, the new messages are appended to it.

## Vault layout

The vault is the parent directory of the inbox. The default inbox is `./inbox`, resolved to an absolute path at startup from the current working directory, so the default vault is that working directory. `--inbox PATH` replaces the inbox. The vault is always the parent of the resolved inbox path. The tool creates the inbox directory if it is missing.

```text
<vault>/
  .pkmagent/
    state.json
    import.lock
  .pkmagent/secrets/
    gmail-client.json
    gmail.json
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
- A source directory is created only when the tool creates a file there.
- A day directory is created only when the tool creates a file in it.
- Files are UTF-8 Markdown with LF line endings and a trailing newline.
- The tool writes inbox files and the vault’s `.pkmagent` directory. It creates files, appends to files it created earlier, and deletes leftover temporary files under the inbox whose names start with `.tmp-`.
- The tool does not delete day directories, source directories, `.pkmagent`, or Markdown files whose names do not start with `.tmp-`.
- A file with no frontmatter `id`, and any file outside the day directory being updated, stays as it is.

## Vault config and secrets

The tool creates `<vault>/.pkmagent/` and `<vault>/.pkmagent/secrets/` when needed. The secrets directory is mode `0700`. Secret files are mode `0600`.

| Path | Purpose |
| --- | --- |
| `<vault>/.pkmagent/state.json` | Catch-up cursor. |
| `<vault>/.pkmagent/import.lock` | Exclusive lock while an import is running. |
| `<vault>/.pkmagent/secrets/gmail-client.json` | OAuth client id and secret supplied by the user. Keys: `client_id`, `client_secret`. |
| `<vault>/.pkmagent/secrets/gmail.json` | Refresh token for the single authorized Gmail account. |

`state.json` contains one field:

```json
{"last_success_at": "2026-09-27T18:04:11-07:00"}
```

`last_success_at` is an RFC3339 timestamp with a numeric offset. It is the exclusive end of the last successful catch-up: every item at or after this instant is still eligible, and every item before it is not imported again. Missing file, or a file whose only value is `null`, means there has never been a successful catch-up. A file that exists but is not valid JSON of that shape is an error: the tool writes nothing and exits `1`.

The tool never prints tokens, client secrets, or message bodies in logs. It never writes secrets into day directories or Markdown files. Removing day directories from the inbox does not remove `.pkmagent`.

## Commands

### `pkmagent auth gmail`

Authorizes exactly one Gmail account with OAuth for the scope `https://www.googleapis.com/auth/gmail.readonly`. Stores the credential in `<vault>/.pkmagent/secrets/gmail.json`, replacing any previous account. Does not read or write the inbox and does not change the cursor. `--inbox` selects the vault the same way it does for import.

The Gmail client file must already exist at `<vault>/.pkmagent/secrets/gmail-client.json`. If it is missing, the command exits `1`.

### `pkmagent import`

Catches up from the cursor through the moment the run starts.

- No cursor: `--since YYYY-MM-DD` is required. The window starts at local midnight at the beginning of that date.
- Cursor present: `--since` is rejected. The window starts at `last_success_at`.
- The window ends at the current time, frozen when the run starts. Items that arrive while the run is in progress wait for the next run.
- `--since` whose midnight is after that frozen end is rejected.
- `--day` together with `--since` is rejected.

An item is included when its instant `t` satisfies `window_start <= t < window_end`. On success the cursor becomes `window_end`. Days are published from oldest to newest. A later day is not published in this run if an earlier day failed.

### `pkmagent import --day YYYY-MM-DD`

Appends missing items for that one civil day and does not change the cursor, whether the date is in the past, today, or the future. The slice is the full local day, from midnight inclusive to the next midnight exclusive. `--since` is not used. This can import items the cursor has already passed. It does not delete files or sections.

### Shared rules

- `--inbox PATH` is optional on import commands.
- Invalid arguments, a corrupt state file, a missing inbox parent that cannot be created, or an existing lock: write nothing, leave the cursor unchanged, exit `1`.
- One import at a time. At start, if `<vault>/.pkmagent/import.lock` exists, exit `1` and tell the user to delete the lock if no import is running. Remove the lock when the process exits. A crash can leave the lock behind.
- Overlapping imports are not supported. There is no `--force`.
- If the frozen end is earlier than `last_success_at`, exit `1` and write nothing. If they are equal, the window is empty: exit `0`, write nothing, and leave the cursor unchanged.

There is no `--from`, `--to`, or `--only`. Every import reads Gmail, Messages, and Calendar together for each day.

## Publishing a day

The tool fetches the whole slice from all three sources before it writes. A fetch failure changes nothing in that day directory.

The catch-up visits every local civil date from the date of `window_start` through the date of the last instant inside the window. Iterate civil dates, not 24-hour blocks. A DST transition must not skip or repeat a calendar date. An empty window visits no dates.

For civil date `D`, the slice is the intersection of the window with that date:

- `slice_start` is the later of `window_start` and local midnight at the start of `D`.
- `slice_end` is the earlier of `window_end` and local midnight at the start of `D+1`.
- Include items with `slice_start <= t < slice_end`.

A catch-up that starts at 10:00 and runs again at 18:00 visits only today. The slice is 10:00 inclusive through 18:00 exclusive. Mail from 11:00 is included. Mail from 09:00 is not.

1. At startup, delete every file or directory under the inbox whose name starts with `.tmp-`.
2. Fetch Gmail, Messages, and Calendar for the slice. The three fetches run in parallel with other days, as defined under Parallel import. If any fetch for this day fails, do not write this day or any later day, and stop the run. The cursor stays where the previous successful slice left it.
3. If the slice has no items, write nothing. Count the slice as published.
4. Otherwise append or create files as defined below.
5. On catch-up, after every write for the slice has succeeded, set `last_success_at` to `slice_end`. If that is still before `window_end`, continue to the next date. If it equals `window_end`, the run is finished.
6. `--day` uses steps 2 through 4 and does not change the cursor. An empty `--day` slice writes nothing and deletes nothing.

A write failure stops the slice. Files already appended in that slice stay appended. The cursor is not advanced, so the next run reads the slice again and skips message ids already in those files. A leftover `.tmp-` file is removed on the next startup.

The first day of a catch-up failing leaves a missing cursor missing, and leaves an existing cursor unchanged. A failure on a later day keeps the cursor at that day's `slice_start`, which is local midnight when the previous day was fully published.

## Parallel import

A job is one source fetch for one civil date: Gmail, Messages, or Calendar. A window of three days has nine jobs. `--day` has three.

Fetches run concurrently. At most one fetch per source runs at a time, so at most three jobs run together. Messages stays a single read of `chat.db`. Gmail stays a single request stream. Calendar stays a single EventKit query stream. The next date for a source starts only after that source's earlier date has finished.

Writes do not run in parallel. A day's files are appended only after all three of its fetches have succeeded and every earlier day in the window has been published. Results for a later day sit in memory until then.

If a job fails, the tool starts no further jobs. In-flight jobs may finish, and their results are discarded. The failed day is not written. Later days are not written.

## Progress

Import shows one progress bar on stderr. It is one line, redrawn with a carriage return. It is not a full-screen interface: no alternate screen, no panels, and no mouse.

The line is:

```text
import <done>/<total> [<bar>] <source> <YYYY-MM-DD>
```

`<total>` is the number of jobs. `<done>` is how many of those jobs have finished successfully. `<bar>` is 20 columns. The number of `=` characters is `floor(20 * done / total)`. When `done` is less than `total`, the next column is `>`. The remaining columns are spaces. When every job has finished, the bar is 20 `=` characters. The label is the source and date of the job that most recently finished. Before the first job finishes, the label is `starting`.

```text
import  4/9 [========>           ] messages 2026-09-27
```

Draw this line only when stderr is a terminal. When stderr is not a terminal, print no progress line. The per-day stdout summary is unchanged either way. After the last update, write a newline so the next stderr line is not appended to the bar. A failed job stops the bar, then the failure line follows on stderr.

## Append and create

Look only in `inbox/YYYY-MM-DD/<source>/`. Ignore files whose names start with `.tmp-`. Read frontmatter `id` from each `.md` file in that directory.

### Gmail and Messages

Match the Gmail thread id or the Messages chat id.

- If one or more files have that id, append to the file whose path sorts first by raw UTF-8 bytes. Do not create another file for that id.
- Append only sections whose message id is not already in the file. Message ids are the values of `- Message-Id:` lines in Gmail files and `- Id:` lines in Messages files, compared as exact strings.
- The filename stays. Bytes already in the file stay, including frontmatter and any notes. Appended text is an exact suffix. If the file is non-empty and does not end in a newline, the suffix starts with one newline. Then come the new sections in chronological order, in the same Markdown as a new file, with a blank line before each `##` heading. The suffix ends with a newline.
- A changed subject or a new participant is recorded only in the appended sections. Frontmatter is not rewritten. `imported_at` stays at the time the file was created.
- If every new message id is already in the file, leave the file unchanged.
- If no file has that id, create one. The new file contains only the messages from this slice.

### Calendar

If any file in the day's `calendar/` directory has that occurrence id, do not modify it and do not create another file for that occurrence. Do not append to an event file. If no file has that id, create one.

### Files the tool does not touch

A file with no frontmatter `id` is never modified. It still occupies its filename.

### Safe write

Write a new file, or the previous bytes plus the appended sections, to a sibling file whose name starts with `.tmp-`. Rename that file onto the destination only after the full contents are written. A crash before the rename leaves the existing file intact.

## Sources

The tool never sends mail, never edits Messages, and never edits Calendar.

### Gmail

Use the Gmail API with the stored readonly credential. There is one account, the account that `auth gmail` stored.

Include every message whose Gmail `internalDate` falls in the slice, whether it was received or sent, including archived mail. Ignore the `Date` header. Exclude Spam, Trash, and Drafts. The instant used for the window is `internalDate`.

Group the slice's messages by Gmail thread id. Sections for that thread are oldest `internalDate` first. Ties break by Gmail message id, ascending. The `Message-Id` line stores the Gmail API message id. Quoted history inside a body stays. The tool does not fetch messages outside the slice, including earlier messages from the same day that are already in a leftover file.

A thread can have a separate file on each day it has messages. Sent and received messages in the same thread and the same slice go into one file. A later slice the same day appends to that file when it is still in the day's `gmail/` directory. If that file is gone, the later slice creates a new file that contains only the new messages.

If Gmail auth is missing or expired, that day's Gmail fetch fails and the day is not written.

### Messages

Read the local Messages store at `~/Library/Messages/chat.db` read-only. This includes iMessage, SMS, and RCS stored there. The tool needs Full Disk Access. A locked database or a permission failure fails that day's Messages fetch.

Include every chat, including group chats and messages the user sent. Include plain text, attachment-only messages, tapback reactions, and stickers as their own timestamped entries. Omit unsent messages. An unsent message that is already in a file stays there.

The day of a message is its sent timestamp in the local timezone, not the read timestamp. That sent timestamp is the instant used for the window.

Group by chat. Sections are oldest first. Ties break by the database message id, ascending. The same append rule as Gmail applies: a later slice appends to the leftover chat file, or creates a new file when the leftover is gone.

### Calendar

Use the macOS Calendar store (EventKit), read-only. Do not call the Google Calendar REST API. Events appear only if that calendar is enabled in Calendar.app, including iCloud, Exchange, subscribed calendars, and Google accounts synced onto the Mac. Calendars unchecked in Calendar.app are skipped. An account that exists only on the web is out of scope.

Include every returned event: accepted, tentative, not yet responded, declined, and cancelled. Store the status in frontmatter. If the API does not return a cancelled occurrence, do not invent one. An event file that already exists is left unchanged even when the event later changes or is cancelled.

- Timed events use the start instant for the window and are filed on the local civil date of that instant. A multi-day event is one file on the start day. The file records the real end.
- All-day events use the civil start date Calendar.app displays. Do not convert a UTC midnight through the local timezone. For the window only, treat the event as occurring at local midnight at the start of that displayed date. A run later the same day does not import that all-day event again. A multi-day all-day event is one file on its displayed start date. Store inclusive start and inclusive end dates. EventKit's exclusive end date must be converted to the inclusive last day the user sees.
- Recurring events are expanded. One file per occurrence whose window instant falls in the slice.

## Filenames

Filenames are chosen only when creating a file. The tool never renames a file that is already in the inbox.

| Source | Display title for a new file |
| --- | --- |
| Gmail | Subject of the chronologically last message included in the new file, including `Re:` and `Fwd:`. Empty subject becomes `(no subject)`. |
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
7. After a suffix is applied, if the UTF-8 filename including `.md` would exceed 200 bytes, truncate the title further until it fits.

Create new files in ascending raw UTF-8 order of their stable ids. The preferred name is `Title.md`. If that name is already taken, including a case-insensitive match against a file already in the directory or another new file from this slice, use `Title--<suffix>.md`. The suffix is the stable id sanitized with the same character rules, truncated to 40 Unicode scalar values. If that name is also taken, append `-2`, `-3`, and so on. Do not rename the file that already has the preferred name.

Stable ids:

- Gmail: the thread id.
- Messages: the chat guid.
- Calendar: the event identifier, a slash, and the occurrence start (`YYYY-MM-DD` for all-day, RFC3339 for timed).

## Source links

Every new file includes `source_url` in its frontmatter and, under the title heading, a Markdown link to that same URL. An append does not change either one. The link is chosen when the file is created.

### Gmail

```text
https://mail.google.com/mail/?authuser=<account>#all/<thread id>
```

`<account>` is the authorized Gmail address, percent-encoded as a query parameter. `<thread id>` is the Gmail API thread id and is not encoded. The visible link text is `Open in Gmail`.

### Messages

The link opens Messages.app to the participants in the chat. There is no documented URL for a chat guid, so the link addresses the other participants.

1. The scheme is `sms` when the chat guid starts with `SMS;`. Otherwise the scheme is `imessage`.
2. Take every participant handle that is not the user's own account, in frontmatter order.
3. Percent-encode each handle. Encode `+` as `%2B` and `@` as `%40`. Leave the commas that join handles unencoded.
4. If that list is empty, `source_url` is `messages://`. This opens Messages.app and not a specific thread.
5. Otherwise `source_url` is `<scheme>://<handle>,<handle>,...`.

A direct chat uses one handle: `imessage://%2B15551212`. A group chat joins the other participants: `imessage://%2B15551212,ada%40icloud.com`. The visible link text is `Open in Messages`. Use the chat's full participant list, not only people who sent a message in the slice.

### Calendar

```text
ical://ekevent/<utc>/<calendar item id>?method=show&options=more
```

`<utc>` is the occurrence start formatted in UTC as `yyyyMMdd'T'HHmmss'Z'`. An all-day event uses `00:00:00Z` on its displayed start date. `<calendar item id>` is the EventKit calendar item identifier, not the event identifier, and is percent-encoded. Calendar.app does not document this URL. Use it anyway: on current macOS it opens that event. The visible link text is `Open in Calendar`.

When the event also has its own URL, keep that URL on the `URL:` line in the body. Omit that line when it is missing or equal to `source_url`.

## Markdown files

Every new file starts with YAML frontmatter. New-file rendering is deterministic except for `imported_at`, which is the RFC3339 time the file is created. An append does not change frontmatter.

### Gmail

```yaml
---
source: gmail
id: "<thread id>"
title: "<single-line subject>"
source_url: "https://mail.google.com/mail/?authuser=name%40gmail.com#all/<thread id>"
day: YYYY-MM-DD
timezone: America/Los_Angeles
imported_at: 2026-09-27T18:04:11-07:00
account: name@gmail.com
participants:
  - name: "Ada Lovelace"
    email: ada@example.com
---
```

`timezone` is the IANA name of the machine's local zone. On a new file, `participants` is the union of From, To, Cc, and Bcc on the messages in that file, unique by email address, sorted by email. Use an empty name when the display name is unknown. `title` is the display title used for the new file. Later participants appear in appended sections and are not added to this list.

When the message has an HTML body, convert that HTML to Markdown and use the Markdown as the section body. Keep paragraphs, line breaks, headings, lists, emphasis, links, and block quotes. Render a link as `[label](url)`, using the url as the label when the label is empty. Render `blockquote` and `div.gmail_quote` as Markdown quotes. Drop `script` and `style`. Do not download attachments or remote images. An `img` whose `src` is `http` or `https` becomes `![alt](src)`, with empty alt when the image has none. Leave a `cid:` image out of the body; its file still appears in the attachment list.

If the message has no HTML, or the conversion is only whitespace and a `text/plain` body exists, use `text/plain` as the section body. If neither body yields text, the section body is empty. Quoted history stays in whichever body is used.

```markdown
# Quarterly plan

[Open in Gmail](https://mail.google.com/mail/?authuser=name%40gmail.com#all/<thread id>)

## 2026-09-26 08:14:03 -0700 — Ada Lovelace <ada@example.com>

- Message-Id: 18c2f0a1b2c3d4e5
- Subject: Quarterly plan
- To: you@example.com
- Cc: team@example.com

Body of the message.

### Attachments

- plan.pdf (application/pdf)
```

`Message-Id` is the Gmail API message id. Omit `Cc` and `Bcc` when empty. Omit the attachments section when there are none. An attachment with no filename is `unnamed`. List the media type when the source provides it. Order messages oldest first. A changed subject on a later message is a new `Subject` line on that message's section.

### Messages

```yaml
---
source: messages
id: "<chat guid>"
title: "Ada Lovelace"
source_url: "imessage://%2B15551212"
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

On a new file, participants are the chat participants sorted by handle, raw UTF-8 order. The user's own account has `self: true`. Outgoing sender label is `Me`.

```markdown
# Ada Lovelace

[Open in Messages](imessage://%2B15551212)

## 2026-09-27 09:01:00 -0700 — Ada Lovelace

- Id: 48211

See you there.

### Attachments

- photo.jpg (image/jpeg)
```

`Id` is the Messages database message id. Reactions and stickers use the same heading and `Id` line. A reaction entry's body is one line:

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

`status` is one of `accepted`, `declined`, `tentative`, `cancelled`, `needs_action`, `unknown`. Use `needs_action` when the user has not responded. Omit `location` when empty. Omit `attendees` when there are none. Sort attendees by email, then name.

Timed `start` and `end` are RFC3339 in the event's time zone. All-day `start` and `end` are inclusive civil dates (`YYYY-MM-DD`) as Calendar.app shows them. `all_day` is `true` or `false`.

```markdown
# Dentist

[Open in Calendar](ical://ekevent/20260927T220000Z/ABC123?method=show&options=more)

Cleaning.

URL: https://example.com/appointment
```

Omit the description or the URL line when the source has none.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Every requested slice was published. A slice with zero items, or whose items were already in leftover files, is success. |
| `1` | Invalid arguments, corrupt state, missing OAuth client on `auth`, clock went backward, or the lock exists. No file was created or appended. |
| `2` | A requested source failed. Earlier slices in a catch-up window may already have been published. A fetch failure leaves that day unchanged. A write failure may leave earlier appends from that slice in place. |

Stdout gets one line per published slice: `YYYY-MM-DD gmail=<files> messages=<files> calendar=<files>`. `<files>` counts files created or appended, not files left unchanged and not individual messages. Stderr gets the progress bar while the run is going, then `YYYY-MM-DD failed <source>: <reason>` for a failed slice.

## Edge cases

- Leftover files stay. The tool appends by frontmatter `id` inside that day and source directory. It does not search the rest of the inbox.
- The tool does not rewrite existing sections, rewrite frontmatter, rename files, or delete directories.
- Two new threads titled `Hello` in one slice: the lower stable id gets `Hello.md` when that name is free. The other gets `Hello--<stable-id>.md`. An existing `Hello.md` keeps that name, and the new file gets the suffix.
- A subject or contact change does not rename the file. The new subject is the `Subject` line of the appended Gmail section. A new Messages participant appears as the sender of the appended section.
- An unsent message, a message moved to Trash, or a changed event is not removed from an existing file. New unsent messages are omitted. A calendar occurrence that already has a file is not written again.
- A message or event before `last_success_at` is not imported by catch-up. `--day` appends items from that civil day that are not already in a matching file, and it leaves the cursor where it is.
- An event whose start is at or after the frozen end is not imported by catch-up. A timed event later the same day is imported when a later run's window covers its start.
- Changing the machine timezone can change which civil date an instant falls on. The next publish uses the timezone in effect at that run. All-day events still use the date Calendar.app displays.
- Mail that arrived during the slice and was later archived is included. Spam, Trash, and Drafts are not.
- HTML mail is stored as Markdown. A message whose HTML conversion is empty uses its plain-text body. A message with only plain text stays plain text.
- An `internalDate` is included only when it falls inside the slice. A timestamp after the frozen end waits for the next run.
- The same Gmail thread on Monday and Tuesday becomes two files. Each file receives only the messages whose timestamps fall on that day.
- A Wednesday–Friday event becomes one file in Wednesday's folder. The frontmatter end is Friday. A later run does not append to that file.
- An all-day event on March 1 stays in `inbox/2026-03-01/` even when the store encodes all-day events as UTC midnight.
- A Gmail fetch failure writes nothing for that day, including Messages and Calendar. A crash while renaming an appended file leaves the previous file in place.
- A second catch-up the same day, with no items at or after the cursor, writes nothing and advances the cursor to the new frozen end.
- A message at 15:00 is absent from a 10:00 run and present after an 18:00 run. A message at 09:00 stays in the section written by the run whose window covered 09:00.
- A new file's `source_url` and its `Open in` link stay in place when later messages are appended. Deleting day directories does not delete `<vault>/.pkmagent` or `<vault>/.pkmagent/secrets`.

## Worked example

The machine timezone is `America/Los_Angeles`. There is no cursor. The user runs:

```text
pkmagent import --since 2026-09-26
```

at 10:00 on 2026-09-27.

The window is `2026-09-26T00:00:00-07:00` inclusive through `2026-09-27T10:00:00-07:00` exclusive. The run creates files for `2026-09-26` and for items on `2026-09-27` before 10:00. A thread with mail on both days produces two Gmail files. A message at 09:00 is in today's file. A message at 11:00 is not in this run. A dentist appointment that runs from 2026-09-30 through 2026-10-02 is not in this run. After success, `last_success_at` is `2026-09-27T10:00:00-07:00`.

The user leaves today's Gmail file in the inbox.

At 18:00 the same day the user runs `pkmagent import`. The window is `2026-09-27T10:00:00-07:00` inclusive through `2026-09-27T18:00:00-07:00` exclusive. The 11:00 message is appended to the existing thread file. The 09:00 section and the filename stay as they were. The cursor becomes `2026-09-27T18:00:00-07:00`.

If the user had removed that thread file before 18:00, the run would create a new file containing only the messages from 10:00 onward.

## Out of scope

- Any source other than Gmail, the local Messages store, and the local Calendar store.
- Writing back to Gmail, Messages, or Calendar.
- Downloading attachment bytes.
- More than one Gmail account.
- Calling the Google Calendar REST API.
- A sync database inside the inbox.
- Rewriting a file from the source, renaming it, or removing sections already in it.
- Emptying the inbox after a successful import.
- Choosing an implementation language.
