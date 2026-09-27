# pkmagent

`pkmagent` is a local read-only importer. It pulls Gmail, macOS Messages, and macOS Calendar into a Markdown inbox that a personal management system then handles.

This file is the shared behavior. Each source has its own spec:

- [gmail.md](gmail.md)
- [messages.md](messages.md)
- [calendar.md](calendar.md)
- [contacts.md](contacts.md)

Those files define what each source fetches, how items become files, filenames, source links, and file bodies. Gmail, Messages, and Calendar run during `pkmagent import`. Contacts is a separate one-time command and is not part of that catch-up. This file defines the vault, the cursor, publishing, parallelism, progress, and the shared note format. It does not choose a programming language.

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
  contacts/
    Ada Lovelace.md
```

- `YYYY-MM-DD` is the calendar day the item happened, in the machine's local timezone at the moment of that import. It is not the day the import started.
- Source directory names are exactly `gmail`, `messages`, and `calendar`.
- A source directory is created only when the tool creates a file there.
- A day directory is created only when the tool creates a file in it.
- Files are UTF-8 Markdown with LF line endings and a trailing newline.
- The tool writes inbox files, `<vault>/contacts` when `import contacts` runs, and the vault’s `.pkmagent` directory. It creates files, appends to inbox files it created earlier, and deletes leftover temporary files under the inbox whose names start with `.tmp-`.
- The tool does not delete day directories, source directories, `.pkmagent`, or Markdown files whose names do not start with `.tmp-`, except the contacts snapshot in [contacts.md](contacts.md).
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

Gmail authorization is specified in [gmail.md](gmail.md).

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

### `pkmagent import contacts`

Imports the macOS Contacts address book once into `<vault>/contacts`, which is `./contacts` when the vault is the working directory. It does not read or write the inbox and does not change the catch-up cursor. `pkmagent import` and `pkmagent import --day` do not import contacts. Behavior is in [contacts.md](contacts.md).

### Shared rules

- `--inbox PATH` is optional on import commands.
- Invalid arguments, a corrupt state file, a missing inbox parent that cannot be created, or an existing lock: write nothing, leave the cursor unchanged, exit `1`.
- One import at a time. At start, if `<vault>/.pkmagent/import.lock` exists, exit `1` and tell the user to delete the lock if no import is running. Remove the lock when the process exits. A crash can leave the lock behind.
- Overlapping imports are not supported. There is no `--force`.
- If the frozen end is earlier than `last_success_at`, exit `1` and write nothing. If they are equal, the window is empty: exit `0`, write nothing, and leave the cursor unchanged.

There is no `--from`, `--to`, or `--only`. `pkmagent import` and `pkmagent import --day` read Gmail, Messages, and Calendar together for each day. They do not read Contacts.

## Publishing a day

The tool fetches the whole slice from all three sources before it writes. A fetch failure changes nothing in that day directory.

The catch-up visits every local civil date from the date of `window_start` through the date of the last instant inside the window. Iterate civil dates, not 24-hour blocks. A DST transition must not skip or repeat a calendar date. An empty window visits no dates.

For civil date `D`, the slice is the intersection of the window with that date:

- `slice_start` is the later of `window_start` and local midnight at the start of `D`.
- `slice_end` is the earlier of `window_end` and local midnight at the start of `D+1`.
- Include items with `slice_start <= t < slice_end`.

Which instant `t` an item uses is defined in that source's spec.

A catch-up that starts at 10:00 and runs again at 18:00 visits only today. The slice is 10:00 inclusive through 18:00 exclusive. Mail from 11:00 is included. Mail from 09:00 is not.

1. At startup, delete every file or directory under the inbox whose name starts with `.tmp-`.
2. Fetch Gmail, Messages, and Calendar for the slice. The three fetches run in parallel with other days, as defined under Parallel import. If any fetch for this day fails, do not write this day or any later day, and stop the run. The cursor stays where the previous successful slice left it.
3. If the slice has no items, write nothing. Count the slice as published.
4. Otherwise append or create files as defined below and in the source specs.
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

Import shows three progress bars on stderr, one per source, in this order: `gmail`, `messages`, `calendar`. Each bar is one line. This is not a full-screen interface: no alternate screen, no panels, and no mouse.

Each line is:

```text
<source> <done>/<total> [<bar>] <label>
```

`<source>` is padded with spaces to 8 columns. `<total>` is the number of civil dates in the window for that source. `--day` uses a total of 1. `<done>` is how many of that source's jobs have finished successfully. `<bar>` is 20 columns. The number of `=` characters is `floor(20 * done / total)`. When `done` is less than `total` and the source has started, the next column is `>`. The remaining columns are spaces. When every job for that source has finished, the bar is 20 `=` characters. `<label>` is the date of the job that most recently finished for that source. If none have finished and one is running, the label is that running date. If the source has not started, the bar is 20 spaces and the label is `waiting`.

```text
gmail    2/3 [=============>      ] 2026-09-27
messages 1/3 [======>             ] 2026-09-26
calendar 0/3 [                    ] waiting
```

Draw these lines only when stderr is a terminal. The first paint prints all three. Later paints move the cursor up three lines and rewrite them. When stderr is not a terminal, print no progress lines. The per-day stdout summary is unchanged either way. After the last paint, the cursor sits on the line below the calendar bar. A failed job stops the bars, then the failure line follows on stderr.

`pkmagent import contacts` does not use these three bars. It shows one bar, specified in [contacts.md](contacts.md).

## Append and create

Look only in `inbox/YYYY-MM-DD/<source>/`. Ignore files whose names start with `.tmp-`. Read frontmatter `id` from each `.md` file in that directory.

Gmail and Messages append new sections to a leftover file with the same id. Calendar creates a file only when that occurrence id is absent. The match keys, message ids, and section format are in the source specs.

Rules that apply to every source:

- A file with no frontmatter `id` is never modified. It still occupies its filename.
- The filename stays when an existing file is updated. Frontmatter is not rewritten. `imported_at` and `source_url` stay as they were when the file was created.
- Bytes already in the file stay, including any notes. Appended text is an exact suffix. If the file is non-empty and does not end in a newline, the suffix starts with one newline. Then come the new sections in chronological order, in the same Markdown as a new file, with a blank line before each `##` heading. The suffix ends with a newline.
- If every new item is already represented in the matching file, leave the file unchanged.
- Write a new file, or the previous bytes plus the appended sections, to a sibling file whose name starts with `.tmp-`. Rename that file onto the destination only after the full contents are written. A crash before the rename leaves the existing file intact.

The tool never sends mail, never edits Messages, and never edits Calendar.

## Filenames

Filenames are chosen only when creating a file. The tool never renames a file that is already in the inbox. The display title and stable id come from the source spec.

Sanitizing a title:

1. Collapse all whitespace, including newlines, to a single space and trim the ends.
2. If the result is empty, use the empty-title fallback from the source spec.
3. Replace `\ / : * ? " < > |` and ASCII control characters with `-`.
4. Trim trailing spaces and dots.
5. If the result is empty, `.`, or `..`, use `untitled`.
6. Truncate to 80 Unicode scalar values.
7. After a suffix is applied, if the UTF-8 filename including `.md` would exceed 200 bytes, truncate the title further until it fits.

Create new files in ascending raw UTF-8 order of their stable ids. The preferred name is `Title.md`. If that name is already taken, including a case-insensitive match against a file already in the directory or another new file from this slice, use `Title--<suffix>.md`. The suffix is the stable id sanitized with the same character rules, truncated to 40 Unicode scalar values. If that name is also taken, append `-2`, `-3`, and so on. Do not rename the file that already has the preferred name.

## Note format

Every imported note is a UTF-8 Markdown file with LF line endings and a trailing newline. New-file rendering is deterministic except for `imported_at`. An append does not change front matter or the source link.

### Front matter

Every note starts with YAML front matter. These fields are present, in this order:

```yaml
---
source: gmail
id: "<stable id>"
title: "<single line>"
source_url: "<url>"
imported_at: 2026-09-27T18:04:11-07:00
```

`source` is `gmail`, `messages`, `calendar`, or `contacts`. `id` is the stable id from that source spec. `title` is the display title, one line. `source_url` is the link defined by that source spec. `imported_at` is the RFC3339 time the file was created, with a numeric offset.

Gmail, Messages, and Calendar notes then include:

```yaml
day: YYYY-MM-DD
timezone: America/Los_Angeles
```

`day` is the inbox folder date. `timezone` is the IANA name of the machine's local zone. Contacts notes omit `day` and `timezone`.

Any further fields are defined only in the source spec and come after these shared fields. Omit an optional source field when the source has no value. Do not repeat a shared field under another name.

### Body

The body is Markdown. Use Markdown for structure that the source can support: headings, lists, links, emphasis, and quotes. When the source has only plain text, keep that plain text. Do not wrap the body in a code fence.

The body starts with the title and the source link:

```markdown
# Quarterly plan

[Open in Gmail](https://mail.google.com/mail/?authuser=name%40gmail.com#all/<thread id>)
```

The heading text is `title`. The link URL is `source_url`. The link text is `Open in Gmail`, `Open in Messages`, `Open in Calendar`, or `Open in Contacts`, matching `source`. The source spec defines how to build `source_url` and does not repeat this layout.

People, phone numbers, and email addresses that match a contact file are wiki links, as defined below. An address that does not match stays as the source wrote it.

When a note lists attachments, they are a Markdown list under `### Attachments`. Each item is `filename (media type)`. A missing filename is `unnamed`. Omit the media type when the source has none. Omit the heading when there are no attachments. Do not download attachment bytes.

### Contact wiki links

Gmail, Messages, and Calendar read `<vault>/contacts` when they write a new file or append a section. They do not rewrite sections or filenames that already exist. If `contacts/` is missing, or a later `import contacts` adds a match, existing inbox text stays as it was.

Build the lookup from each contact file that has a frontmatter `id`:

- The contact title is the frontmatter `title`.
- The link target is that file's name without `.md`.
- Phone values are the text after `:` on each list item under `## Phones` in [contacts.md](contacts.md).
- Email values are the text after `:` on each list item under `## Emails` there.

Match an email by trimming whitespace and comparing case-insensitively. Match a phone by its digits only. Two phones match when the digit strings are equal, or when the longer one is the shorter one with a single leading `1` and the shorter one has 10 digits. Do not match any other partial overlap.

When several contact files match, use the one whose filename sorts first by raw UTF-8 bytes.

The wiki link is `[[<target>]]` when the filename stem equals the title. When sanitizing changed the name, it is `[[<target>|<title>]]`. In front matter, store the contact title as a plain string, not as a wiki link. In the body, a matched person is the wiki link. Where the source also has an email address, write `[[Ada Lovelace]] <ada@example.com>`. A sender that is the user, with no contact match, stays `Me`.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Every requested slice was published. A slice with zero items, or whose items were already in leftover files, is success. |
| `1` | Invalid arguments, corrupt state, missing OAuth client on `auth`, clock went backward, or the lock exists. No file was created or appended. |
| `2` | A requested source failed. Earlier slices in a catch-up window may already have been published. A fetch failure leaves that day unchanged. A write failure may leave earlier appends from that slice in place. |

Stdout gets one line per published slice: `YYYY-MM-DD gmail=<files> messages=<files> calendar=<files>`. `<files>` counts files created or appended, not files left unchanged and not individual messages. Stderr gets the three progress bars while the run is going, then `YYYY-MM-DD failed <source>: <reason>` for a failed slice.

## Edge cases

- Leftover files stay. The tool appends by frontmatter `id` inside that day and source directory. It does not search the rest of the inbox.
- The tool does not rewrite existing sections, rewrite frontmatter, rename files, or delete directories.
- Two new files with the same sanitized title in one slice: the lower stable id gets `Title.md` when that name is free. The other gets `Title--<stable-id>.md`. An existing `Title.md` keeps that name, and the new file gets the suffix.
- A message or event before `last_success_at` is not imported by catch-up. `--day` appends items from that civil day that are not already in a matching file, and it leaves the cursor where it is.
- Changing the machine timezone can change which civil date an instant falls on. The next publish uses the timezone in effect at that run.
- A fetch failure for one source writes nothing for that day, including the other sources. A crash while renaming an appended file leaves the previous file in place.
- A second catch-up the same day, with no items at or after the cursor, writes nothing and advances the cursor to the new frozen end.
- A message at 15:00 is absent from a 10:00 run and present after an 18:00 run. A message at 09:00 stays in the section written by the run whose window covered 09:00.
- A new file's `source_url` and its open link stay in place when later messages are appended. Deleting day directories does not delete `<vault>/.pkmagent` or `<vault>/.pkmagent/secrets`.

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

- Any source other than Gmail, the local Messages store, the local Calendar store, and the local Contacts store.
- Writing back to Gmail, Messages, or Calendar.
- Downloading attachment bytes.
- More than one Gmail account.
- Calling the Google Calendar REST API.
- A sync database inside the inbox.
- Rewriting a file from the source, renaming it, or removing sections already in it.
- Emptying the inbox after a successful import.
- Choosing an implementation language.
