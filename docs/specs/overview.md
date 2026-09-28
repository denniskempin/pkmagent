# pkmagent

`pkmagent` is a local read-only importer. `pkmagent collect` pulls Gmail, macOS Messages, and macOS Calendar into a Markdown inbox. `pkmagent import-contacts` writes `<vault>/contacts` and is not part of collect.

- [gmail.md](gmail.md)
- [messages.md](messages.md)
- [calendar.md](calendar.md)
- [contacts.md](contacts.md)

Source specs define what to fetch, stable ids, filenames, source URLs, and extra fields. This file defines the vault, the cursor, a collect run, and the shared note format.

## Workflow

The inbox is a drop zone. The tool does not empty it. Each collect writes a new folder and does not read or modify older ones. Catch-up state lives in `<vault>/.pkmagent`, outside the inbox.

The cursor is a timestamp, the exclusive end of the last successful collect. A run at 18:00 after a 10:00 run collects only items from 10:00 onward, into a new folder.

## Vault layout

The vault is the parent of the inbox. The default inbox is `./inbox`, so the default vault is the working directory. `--inbox PATH` replaces the inbox. The tool creates the inbox if it is missing.

A collect names its folder from the run's start time, local offset, as `YYYY-MM-DDTHHMMSS±HHMM` with the colons removed: `2026-09-27T100000-0700`. If that name exists, append `-2`, `-3`, and so on.

The folder holds `gmail/`, `messages/`, and `calendar/` directly when the run covers one or two civil dates. When it covers more than two, those source directories sit under a `YYYY-MM-DD/` layer, one per civil date that receives a file.

```text
<vault>/
  .pkmagent/
    state.json
    collect.lock
  .pkmagent/secrets/
    gmail-client.json
    gmail.json
  inbox/
    2026-09-27T100000-0700/
      gmail/
        Quarterly plan.md
      messages/
        Ada Lovelace.md
      calendar/
        Dentist.md
    2026-09-25T100000-0700/
      2026-09-25/
        gmail/
          Quarterly plan.md
      2026-09-27/
        calendar/
          Dentist.md
  contacts/
    Ada Lovelace.md
```

A civil date is the local day the item happened, not the day the collect started. A timezone change can move an instant onto another civil date. A later run uses the zone in effect then.

Collect writes only its new inbox folder and `<vault>/.pkmagent`. It does not delete inbox folders. `import-contacts` writes `<vault>/contacts` only, as [contacts.md](contacts.md) describes.

## Vault config and secrets

Create `<vault>/.pkmagent/` and `<vault>/.pkmagent/secrets/` when needed. The secrets directory is mode `0700`. Secret files are mode `0600`.

| Path | Purpose |
| --- | --- |
| `<vault>/.pkmagent/state.json` | Collect cursor. |
| `<vault>/.pkmagent/collect.lock` | Exclusive lock while collect is running. |
| `<vault>/.pkmagent/contacts.lock` | Exclusive lock while `import-contacts` is running. |
| `<vault>/.pkmagent/secrets/gmail-client.json` | OAuth client supplied by the user. Keys: `client_id`, `client_secret`. |
| `<vault>/.pkmagent/secrets/gmail.json` | Refresh token for the one authorized Gmail account. |

```json
{"last_success_at": "2026-09-27T18:04:11-07:00"}
```

`last_success_at` is RFC3339 with a numeric offset, to the second. A missing file, or a file whose only value is `null`, means collect has never succeeded. Any other shape is corrupt: write nothing and exit `1`.

Do not print tokens, client secrets, or message bodies.

## Commands

### `pkmagent auth gmail`

Authorize one Gmail account for `https://www.googleapis.com/auth/gmail.readonly` and store the refresh token in `gmail.json`, replacing any previous account. The client file must already exist; if it is missing, exit `1`. `--inbox` selects the vault. This command does not change the cursor.

### `pkmagent collect`

Collect from the cursor through the time the run starts. Freeze that end before fetching and floor it to the whole second. Items that arrive during the run, including the rest of that second, wait for the next run.

- No cursor: `--since YYYY-MM-DD` is required. The window starts at local midnight at the start of that date. A `--since` midnight after the frozen end is rejected.
- Cursor present: reject `--since`. The window starts at `last_success_at`.
- Include items with `window_start <= t < window_end`. On success the cursor becomes `window_end`.
- If the frozen end is earlier than the cursor, exit `1` and write nothing. If they are equal, exit `0`, write nothing, and leave the cursor unchanged.

`--day` cannot be combined with `--since`.

### `pkmagent collect --day YYYY-MM-DD`

Collect that one local civil day, midnight inclusive through the next midnight exclusive, into a new folder. Do not change the cursor. This can collect items the cursor has already passed.

### `pkmagent import-contacts`

See [contacts.md](contacts.md). It does not take collect's flags, does not use `collect.lock`, and does not read the cursor.

### Shared rules

`--inbox PATH` is optional on every command.

One collect at a time. If `collect.lock` exists, exit `1` and tell the user to delete it when no collect is running. Remove the lock when the process exits. A crash can leave it. There is no `--force`. `import-contacts` has the same rule for `contacts.lock`.

## A collect run

Visit every local civil date from the date of `window_start` through the date of the last instant inside the window. Step by civil date, not by 24-hour blocks, so a DST transition does not skip or repeat a date. An empty window visits no dates. `--day` visits that one date.

For civil date `D`:

- `slice_start` is the later of `window_start` and local midnight at the start of `D`.
- `slice_end` is the earlier of `window_end` and local midnight at the start of `D+1`.
- Include items with `slice_start <= t < slice_end`.

Each source spec defines the instant `t`. For `--day`, the slice is the whole civil day.

Fetches follow Parallel collect. Write files only after every fetch has succeeded. Write them directly into the new folder. There is no temporary file and no rename into place.

One or two civil dates: files go in `<run>/gmail/`, `<run>/messages/`, and `<run>/calendar/`. A thread or chat is one file for the whole run, and every event occurrence goes in `<run>/calendar/`. More than two civil dates: files go in `<run>/YYYY-MM-DD/<source>/`. A thread or chat is one file per date, and an event occurrence goes under its civil date.

Create the run folder only when there is a file to write. On success, set `last_success_at` to `window_end`. `--day` does not change the cursor.

If a fetch fails, write no folder and leave the cursor unchanged. If a write fails, leave the cursor unchanged. The run folder may already contain files. The next collect does not open it. It starts a new folder for the same window.

## Parallel collect

A job is one source fetch for one civil date. At most one fetch per source runs at a time. The next date for a source starts after that source's earlier date finishes.

If a job fails, start no further jobs and discard in-flight results. Do not write the run folder.

## Progress

When stderr is a terminal, rewrite three status lines in place, in order: `gmail`, `messages`, `calendar`. Each line shows the source, `<done>/<total>` dates, and the date that source last finished or is fetching. Before it starts, show `waiting`. `<total>` is the number of civil dates in the window, or `1` for `--day`.

```text
gmail    2/3 [=============>      ] 2026-09-27
messages 1/3 [======>             ] 2026-09-26
calendar 0/3 [                    ] waiting
```

When stderr is not a terminal, print no progress. On failure, print `YYYY-MM-DD failed <source>: <reason>` to stderr either way.

`import-contacts` uses one line of the same shape. See [contacts.md](contacts.md).

## Files

A collect only creates files. One file per stable id in the directory it is written to. Gmail and Messages put every item for that id in that directory into the one file, oldest first.

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

Create files in ascending raw UTF-8 order of stable id, so collisions are deterministic. Prefer `Title.md`. If that name is taken, including a case-insensitive match in the directory or among files created in this directory, use `Title--<suffix>.md`. The suffix is the stable id sanitized the same way, truncated to 40 Unicode scalar values. If that name is taken, append `-2`, `-3`, and so on.

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

`source` is `gmail`, `messages`, `calendar`, or `contacts`. `imported_at` is RFC3339 with a numeric offset, set when the file is created. `day` is the civil date of the earliest item in the file. In a dated run that is the date folder. `timezone` is the machine's IANA zone. Contacts notes omit `day` and `timezone`.

Source specs add fields after these. Omit an optional field when the source has no value.

### Body

The body does not repeat the title as a heading. It starts with the source link. The URL is `source_url`. The link text is `Open in Gmail`, `Open in Messages`, `Open in Calendar`, or `Open in Contacts`.

```markdown
[Open in Gmail](https://mail.google.com/mail/?authuser=name%40gmail.com#all/<thread id>)
```

Attachments, when present, are a list under `### Attachments`. Each item is `filename (media type)`. A missing filename is `unnamed`. Omit the media type when unknown. Do not download attachment bytes.

### Contact wiki links

When collect writes a file, it resolves addresses against `<vault>/contacts`. It does not write that directory. If `contacts/` is missing, leave addresses as the source wrote them.

Use contact files that have a frontmatter `id`. The contact title is the filename without `.md`. Match `phones[].value` and `emails[].value` from that file's front matter. See [contacts.md](contacts.md).

Compare emails trimmed, case-insensitive. Compare phones by digits only. A match is equal digits, or the longer number is the shorter one plus a single leading `1` when the shorter one has 10 digits. No other partial match. Several matches: use the filename that sorts first by raw UTF-8 bytes.

In the body, a matched person is `[[<title>]]`. With an email, write `[[Ada Lovelace]] <ada@example.com>`. In front matter, store that title as a plain string. The user's own unmatched sender stays `Me`.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The run finished. A window with nothing to write is success. |
| `1` | Bad arguments, corrupt state, missing OAuth client, clock went backward, or the command's lock exists. Nothing was written. |
| `2` | A source failed. The cursor is unchanged. No run folder is left when the failure was a fetch. A write failure can leave that run's folder in place. |

Stdout begins with the run folder path when one was created. Then one line: `gmail=<files> messages=<files> calendar=<files>`. A dated run prints that line once per date that received a file, prefixed with `YYYY-MM-DD`. An empty success prints `gmail=0 messages=0 calendar=0` and no folder path.

## Worked example

Timezone `America/Los_Angeles`, no cursor. At 10:00 on 2026-09-27:

```text
pkmagent collect --since 2026-09-26
```

The window is `2026-09-26T00:00:00-07:00` inclusive through `2026-09-27T10:00:00-07:00` exclusive. That is two civil dates, so the folder is flat:

```text
inbox/2026-09-27T100000-0700/
  gmail/
  messages/
  calendar/
```

A thread with mail on both days becomes one Gmail file containing both messages. A message at 11:00 is not included. A dentist appointment on 2026-09-30 is not included. `last_success_at` becomes `2026-09-27T10:00:00-07:00`. The morning folder stays as it was.

At 18:00, `pkmagent collect` covers 10:00 inclusive through 18:00 exclusive. That is one civil date, written to a new folder `inbox/2026-09-27T180000-0700/`. The 11:00 message is a new file there. The 10:00 folder is not opened.

The same morning command with `--since 2026-09-25` covers three civil dates, so each file is under `2026-09-25/`, `2026-09-26/`, or `2026-09-27/` inside the run folder. The thread's Monday mail and Tuesday mail are two files.

## Out of scope

- Sources other than Gmail, the local Messages database, EventKit, and the Contacts framework.
- The Google Calendar REST API.
- More than one Gmail account.
- A separate database of collected ids. The cursor is the record.
- Resuming or deleting a run folder left behind by a crashed collect.
