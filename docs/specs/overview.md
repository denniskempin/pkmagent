# pkmagent

`pkmagent` is a local read-only importer. It pulls Gmail, macOS Messages, and macOS Calendar into a Markdown inbox. Contacts is a separate import into `<vault>/contacts` and is not part of catch-up.

- [gmail.md](gmail.md)
- [messages.md](messages.md)
- [calendar.md](calendar.md)
- [contacts.md](contacts.md)

Source specs define what to fetch, stable ids, filenames, source URLs, and extra fields. This file defines the vault, the cursor, publishing, and the shared note format.

## Workflow

The inbox is a drop zone. The tool does not empty it. Leftover files stay, and the next import appends to a leftover file with the same id, or creates a file when none is left. Catch-up state lives in `<vault>/.pkmagent`, outside the inbox, so emptying day folders does not repeat items.

The cursor is a timestamp, the exclusive end of the last successful catch-up. A run at 18:00 after a 10:00 run imports only items from 10:00 onward.

## Vault layout

The vault is the parent of the inbox. The default inbox is `./inbox`, so the default vault is the working directory. `--inbox PATH` replaces the inbox. The tool creates the inbox if it is missing.

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

`YYYY-MM-DD` is the local civil day the item happened, not the day the import started. A timezone change can move an instant to a different civil date; a later run uses the zone in effect then.

The tool writes inbox files, `<vault>/contacts` for `import contacts`, and `<vault>/.pkmagent`. It deletes leftover inbox paths whose names start with `.tmp-`, and contacts files only as [contacts.md](contacts.md) describes. It does not delete other Markdown files or day directories.

## Vault config and secrets

Create `<vault>/.pkmagent/` and `<vault>/.pkmagent/secrets/` when needed. The secrets directory is mode `0700`. Secret files are mode `0600`.

| Path | Purpose |
| --- | --- |
| `<vault>/.pkmagent/state.json` | Catch-up cursor. |
| `<vault>/.pkmagent/import.lock` | Exclusive lock while an import is running. |
| `<vault>/.pkmagent/secrets/gmail-client.json` | OAuth client supplied by the user. Keys: `client_id`, `client_secret`. |
| `<vault>/.pkmagent/secrets/gmail.json` | Refresh token for the one authorized Gmail account. |

```json
{"last_success_at": "2026-09-27T18:04:11-07:00"}
```

`last_success_at` is RFC3339 with a numeric offset. A missing file, or a file whose only value is `null`, means catch-up has never succeeded. Any other shape is corrupt: write nothing and exit `1`.

Do not print tokens, client secrets, or message bodies.

## Commands

### `pkmagent auth gmail`

Authorize one Gmail account for `https://www.googleapis.com/auth/gmail.readonly` and store the refresh token in `gmail.json`, replacing any previous account. The client file must already exist; if it is missing, exit `1`. `--inbox` selects the vault. This command does not change the cursor.

### `pkmagent import`

Catch up from the cursor through the time the run starts. Freeze that end before fetching. Items that arrive during the run wait for the next run.

- No cursor: `--since YYYY-MM-DD` is required. The window starts at local midnight at the start of that date. A `--since` midnight after the frozen end is rejected.
- Cursor present: reject `--since`. The window starts at `last_success_at`.
- Include items with `window_start <= t < window_end`. On success the cursor becomes `window_end`.
- If the frozen end is earlier than the cursor, exit `1` and write nothing. If they are equal, exit `0`, write nothing, and leave the cursor unchanged.

Publish days from oldest to newest. A later day is not published if an earlier day failed. `--day` cannot be combined with `--since`.

### `pkmagent import --day YYYY-MM-DD`

Append missing items for that one local civil day, midnight inclusive through the next midnight exclusive. Do not change the cursor. This can import items the cursor has already passed.

### `pkmagent import contacts`

See [contacts.md](contacts.md).

### Shared rules

`--inbox PATH` is optional on every command.

One import at a time. If `import.lock` exists, exit `1` and tell the user to delete it when no import is running. Remove the lock when the process exits. A crash can leave it. There is no `--force`.

## Publishing a day

Visit every local civil date from the date of `window_start` through the date of the last instant inside the window. Step by civil date, not by 24-hour blocks, so a DST transition does not skip or repeat a date. An empty window visits no dates.

For civil date `D`:

- `slice_start` is the later of `window_start` and local midnight at the start of `D`.
- `slice_end` is the earlier of `window_end` and local midnight at the start of `D+1`.
- Include items with `slice_start <= t < slice_end`.

Each source spec defines the instant `t`.

1. At startup, delete inbox paths whose names start with `.tmp-`.
2. Fetch all three sources for the slice. Fetches follow Parallel import. If one fails, do not write this day or any later day.
3. If the slice has nothing new to write, write nothing.
4. Otherwise append or create, as below and in the source specs.
5. On catch-up, a published slice sets `last_success_at` to `slice_end`. Continue until that equals `window_end`. A slice with nothing new is still published.

`--day` uses steps 2 through 4 for that date and does not change the cursor.

A write failure is not published. Appends already made stay, and the cursor does not advance, so the next run reads that slice again and skips ids already in the files. If nothing in the run has been published, a missing cursor stays missing.

## Parallel import

A job is one source fetch for one civil date. At most one fetch per source runs at a time. The next date for a source starts after that source's earlier date finishes.

Write a day only after its three fetches have succeeded and every earlier day has been published. If a job fails, start no further jobs and discard in-flight results.

## Progress

When stderr is a terminal, rewrite three status lines in place, in order: `gmail`, `messages`, `calendar`. Each line shows the source, `<done>/<total>` dates, and the date that source last finished or is fetching. Before it starts, show `waiting`. `<total>` is the number of civil dates in the window, or `1` for `--day`.

```text
gmail    2/3 [=============>      ] 2026-09-27
messages 1/3 [======>             ] 2026-09-26
calendar 0/3 [                    ] waiting
```

When stderr is not a terminal, print no progress. On failure, stop the bars and print `YYYY-MM-DD failed <source>: <reason>`.

`pkmagent import contacts` uses one line of the same shape. See [contacts.md](contacts.md).

## Append and create

Look only in `inbox/YYYY-MM-DD/<source>/`. Gmail and Messages append sections to a file with the same frontmatter `id`. If several files share that id, append to the path that sorts first by raw UTF-8 bytes. If none remains, create a file that contains only the new items. Calendar does not append; see [calendar.md](calendar.md).

- A file with no frontmatter `id` is never modified. It still occupies its filename.
- An append does not change existing bytes, front matter, or the filename. Skip items the file already contains. Match keys are in the source specs.
- New sections follow the new-file Markdown, oldest first.
- Write the full new contents to a sibling whose name starts with `.tmp-`, then rename it into place.

## Filenames

Choose a filename only when creating a file. The display title and stable id come from the source spec. `title` in front matter is that filename without `.md`, including a collision suffix.

Sanitizing a title:

1. Collapse whitespace to a single space and trim.
2. If empty, use the source spec's empty-title fallback.
3. Replace `\ / : * ? " < > |` and ASCII controls with `-`.
4. Trim trailing spaces and dots.
5. If the result is empty, `.`, or `..`, use `untitled`.
6. Truncate to 80 Unicode scalar values.
7. If the UTF-8 filename including `.md` would exceed 200 bytes after a suffix, truncate the title further until it fits.

Create files in ascending raw UTF-8 order of stable id, so collisions are deterministic. Prefer `Title.md`. If that name is taken, including a case-insensitive match in the directory or among files created in this slice, use `Title--<suffix>.md`. The suffix is the stable id sanitized the same way, truncated to 40 Unicode scalar values. If that name is taken, append `-2`, `-3`, and so on.

## Note format

UTF-8 Markdown, LF, trailing newline. A new file is deterministic except for `imported_at`.

### Front matter

```yaml
---
source: gmail
id: "<stable id>"
title: "<filename without .md>"
source_url: "<url>"
imported_at: 2026-09-27T18:04:11-07:00
day: YYYY-MM-DD
timezone: America/Los_Angeles
```

`source` is `gmail`, `messages`, `calendar`, or `contacts`. `imported_at` is RFC3339 with a numeric offset, set when the file is created. `day` is the inbox folder date. `timezone` is the machine's IANA zone. Contacts notes omit `day` and `timezone`.

Source specs add fields after these. Omit an optional field when the source has no value.

### Body

The body does not repeat the title as a heading. It starts with the source link. The URL is `source_url`. The link text is `Open in Gmail`, `Open in Messages`, `Open in Calendar`, or `Open in Contacts`.

```markdown
[Open in Gmail](https://mail.google.com/mail/?authuser=name%40gmail.com#all/<thread id>)
```

Attachments, when present, are a list under `### Attachments`. Each item is `filename (media type)`. A missing filename is `unnamed`. Omit the media type when unknown. Do not download attachment bytes.

### Contact wiki links

When writing a new inbox file or appending a section, resolve addresses against `<vault>/contacts`. Do not rewrite text already in the inbox. If `contacts/` is missing, leave addresses as the source wrote them.

Use contact files that have a frontmatter `id`. The contact title is the filename without `.md`. Match `phones[].value` and `emails[].value` from that file's front matter. See [contacts.md](contacts.md).

Compare emails trimmed, case-insensitive. Compare phones by digits only. A match is equal digits, or the longer number is the shorter one plus a single leading `1` when the shorter one has 10 digits. No other partial match. Several matches: use the filename that sorts first by raw UTF-8 bytes.

In the body, a matched person is `[[<title>]]`. With an email, write `[[Ada Lovelace]] <ada@example.com>`. In front matter, store that title as a plain string. The user's own unmatched sender stays `Me`.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | Every requested slice was published, including a slice with nothing new to write. |
| `1` | Bad arguments, corrupt state, missing OAuth client, clock went backward, or the lock exists. Nothing was written. |
| `2` | A source failed. Earlier slices in the window may already be published. A fetch failure leaves that day unchanged. A write failure may leave earlier appends from that slice. |

Stdout, one line per published slice: `YYYY-MM-DD gmail=<files> messages=<files> calendar=<files>`. Count files created or appended, not files left unchanged.

## Worked example

Timezone `America/Los_Angeles`, no cursor. At 10:00 on 2026-09-27:

```text
pkmagent import --since 2026-09-26
```

The window is `2026-09-26T00:00:00-07:00` inclusive through `2026-09-27T10:00:00-07:00` exclusive. The run writes `2026-09-26` and items on `2026-09-27` before 10:00. A thread with mail on both days becomes two Gmail files. A message at 11:00 is not included. A dentist appointment on 2026-09-30 is not included. `last_success_at` becomes `2026-09-27T10:00:00-07:00`.

The user leaves today's Gmail file in the inbox. At 18:00, `pkmagent import` covers 10:00 inclusive through 18:00 exclusive. The 11:00 message is appended. The 09:00 section and the filename stay. The cursor becomes `2026-09-27T18:00:00-07:00`.

If that file was removed first, the 18:00 run creates a new file with only the messages from 10:00 onward.

## Out of scope

- Sources other than Gmail, the local Messages database, EventKit, and the Contacts framework.
- The Google Calendar REST API.
- More than one Gmail account.
- A separate database of imported ids. The Markdown files are the record.
