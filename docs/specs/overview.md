# pkmagent

`pkmagent` is a local read-only importer. `collect` writes Gmail, Messages, and Calendar into a new inbox folder. `import` updates Contacts in place under the vault. Contacts have no event date, and a Gmail import with no date window would pull the whole mailbox, so those commands do not cross over.

- [gmail.md](gmail.md)
- [messages.md](messages.md)
- [calendar.md](calendar.md)
- [contacts.md](contacts.md)

Source specs define what to fetch, stable ids, filenames, source URLs, and extra fields.

## Workflow

The inbox is a drop zone. The tool does not empty it. Each collect writes a new folder and does not read or modify older ones. Import does not write the inbox. Collect does not write `<vault>/contacts`.

A collect cursor is a timestamp per category, the exclusive end of the last successful collect of that category. A run at 18:00 after a 10:00 gmail collect collects gmail only from 10:00 onward, into a new folder.

## Vault layout

The vault is the parent of the inbox. The default inbox is `./inbox`, so the default vault is the working directory. `--inbox PATH` replaces the inbox. The tool creates the inbox, and `<vault>/contacts`, when it writes there.

```text
<vault>/
  .pkmagent/
    state.json
    pkmagent.lock
  .pkmagent/secrets/
    gmail-client.json
    gmail.json
  contacts/
    Ada Lovelace.md
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
```

Contact files live directly in `<vault>/contacts/`. One file per person. No day folders.

A collect names its folder from the run's start time, local offset, as `YYYY-MM-DDTHHMMSS±HHMM` with the colons removed: `2026-09-27T100000-0700`. If that name exists, append `-2`, `-3`, and so on.

The folder holds `gmail/`, `messages/`, and `calendar/` when the run covers one or two civil dates. When it covers more than two, those directories sit under a `YYYY-MM-DD/` layer, one per civil date that receives a file. A category with nothing to write gets no directory.

A civil date is the local day the item happened, not the day the run started. A timezone change can move an instant onto another civil date. A later run uses the zone in effect then.

Collect does not delete inbox folders.

## Vault config and secrets

Create `<vault>/.pkmagent/` and `<vault>/.pkmagent/secrets/` when needed. The secrets directory is mode `0700`. Secret files are mode `0600`.

| Path | Purpose |
| --- | --- |
| `<vault>/.pkmagent/state.json` | Collect cursors. |
| `<vault>/.pkmagent/pkmagent.lock` | Exclusive lock while import or collect is running. |
| `<vault>/.pkmagent/secrets/gmail-client.json` | OAuth client supplied by the user. Keys: `client_id`, `client_secret`. |
| `<vault>/.pkmagent/secrets/gmail.json` | Refresh token for the one authorized Gmail account. |

```json
{
  "gmail": "2026-09-27T18:04:11-07:00",
  "messages": null,
  "calendar": null
}
```

Each value is RFC3339 with a numeric offset, floored to the second, or `null`. A missing file, a missing key, or `null` means that category has never been collected. Any other shape is corrupt: write nothing and exit `1`.

Do not print tokens, client secrets, or message bodies.

## Commands

Collect categories are `gmail`, `messages`, and `calendar`. Import's only category is `contacts`. A command with no category runs every category that command allows. Names run in the order above, not the order typed. An unknown name, a name the command does not allow, or the same name twice, is a bad argument.

### `pkmagent auth gmail`

Authorize one Gmail account for `https://www.googleapis.com/auth/gmail.readonly` and store the refresh token in `gmail.json`, replacing any previous account. The client file must already exist; if it is missing, exit `1`. `--inbox` selects the vault. This command does not change cursors.

### `pkmagent import [contacts]`

Update `<vault>/contacts` in place. `pkmagent import` and `pkmagent import contacts` are the same command. `import gmail`, `import messages`, and `import calendar` are rejected. Import has no cursor and takes no `--since` or `--day`. It does not read or write collect cursors.

When a person already has a file, refresh the front matter and leave the body unchanged. Do not rename the file. Details are in [contacts.md](contacts.md).

### `pkmagent collect [category ...]`

Collect each selected category from its cursor through the time the run starts. Freeze that end before fetching and floor it to the whole second. Items that arrive during the run, including the rest of that second, wait for the next run.

- A selected category with no cursor requires `--since YYYY-MM-DD`. The window starts at local midnight at the start of that date. `--since` applies only to those categories. A `--since` midnight after the frozen end is rejected.
- A selected category with a cursor starts at that cursor. If every selected category has a cursor, `--since` is rejected.
- Include items with `window_start <= t < window_end`. On success, each selected category's cursor becomes `window_end`.
- If the frozen end is earlier than any selected cursor, exit `1` and write nothing. A category whose cursor equals the frozen end contributes nothing and still counts as success.

`--day` cannot be combined with `--since`.

### `pkmagent collect [category ...] --day YYYY-MM-DD`

Collect that one local civil day for the selected categories, midnight inclusive through the next midnight exclusive, into a new folder. Do not change cursors. This can collect items a cursor has already passed.

### Shared rules

`--inbox PATH` is optional on every command.

One import or collect at a time. If `pkmagent.lock` exists, exit `1` and tell the user to delete it when nothing is running. Remove the lock when the process exits. A crash can leave it. There is no `--force`.

## A collect run

Visit every local civil date from the date of `window_start` through the date of the last instant inside the window. The window start used for that list is the earliest start among the selected categories. Step by civil date, not by 24-hour blocks, so a DST transition does not skip or repeat a date. An empty window visits no dates. `--day` visits that one date.

For civil date `D` and a category whose own `window_start` is `S`:

- `slice_start` is the later of `S` and local midnight at the start of `D`.
- `slice_end` is the earlier of `window_end` and local midnight at the start of `D+1`.
- Include items with `slice_start <= t < slice_end`.

Each source spec defines the instant `t`. For `--day`, the slice is the whole civil day.

Fetch every selected category before writing. Write files only after every fetch has succeeded. Write them directly into the new folder. There is no temporary file and no rename into place.

One or two civil dates: files go in `<run>/gmail/`, `<run>/messages/`, and `<run>/calendar/`. A thread or chat is one file for the whole run, and every event occurrence goes in `<run>/calendar/`. More than two civil dates: files go in `<run>/YYYY-MM-DD/<category>/`. A thread or chat is one file per date, and an event occurrence goes under its civil date.

Create the run folder only when there is a file to write. On success, set each selected category's cursor to `window_end`. `--day` does not change cursors.

If a fetch fails, write no folder and leave every cursor unchanged. If a write fails, leave every cursor unchanged. The run folder may already contain files. The next collect does not open it.

## Parallel collect

A job is one category fetch for one civil date. At most one fetch per category runs at a time. The next date for a category starts after that category's earlier date finishes.

If a job fails, start no further jobs and discard in-flight results. Do not write the run folder.

## Progress

When stderr is a terminal, rewrite one status line per selected category, in category order. Each line shows the category, `<done>/<total>`, and a label. Before it starts, the label is `waiting`.

`<total>` is the number of civil dates in the window, or `1` for `--day`. The label is the date that category last finished or is fetching.

```text
gmail    2/3 [=============>      ] 2026-09-27
messages 1/3 [======>             ] 2026-09-26
calendar 0/3 [                    ] waiting
```

When stderr is not a terminal, print no progress. On failure, print `YYYY-MM-DD failed <category>: <reason>` to stderr either way.

`pkmagent import` shows one line. `<total>` is the number of people. The label is the display title most recently written, or `waiting`. A failure prints `contacts failed: <reason>`.

## Files

Collect only creates files. One file per stable id in the directory it is written to. Gmail and Messages put every item for that id in that directory into the one file, oldest first.

Import matches a contact file by frontmatter `id` in `<vault>/contacts/`. Several files with the same id: update the path that sorts first by raw UTF-8 bytes.

## Filenames

Choose a filename only when creating a file. Import never renames. The display title and stable id come from the source spec. `title` in front matter is that filename without `.md`, including a collision suffix.

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

`source` is `gmail`, `messages`, `calendar`, or `contacts`. `imported_at` is RFC3339 with a numeric offset, set when the file is created. On a collect file, `day` is the civil date of the earliest item in the file. In a dated collect that is the date folder. `timezone` is the machine's IANA zone. Contact files omit `day` and `timezone`.

Source specs add fields after these. Omit an optional field when the source has no value.

### Body

The body does not repeat the title as a heading. It starts with the source link. The URL is `source_url`. The link text is `Open in Gmail`, `Open in Messages`, `Open in Calendar`, or `Open in Contacts`.

```markdown
[Open in Gmail](https://mail.google.com/mail/?authuser=name%40gmail.com#all/<thread id>)
```

Attachments, when present, are a list under `### Attachments`. Each item is `filename (media type)`. A missing filename is `unnamed`. Omit the media type when unknown. Do not download attachment bytes.

### Contact wiki links

When collect writes a file, it resolves addresses against `<vault>/contacts`. If that directory is missing, leave addresses as the source wrote them.

Use contact files that have a frontmatter `id`. The contact title is the filename without `.md`. Match `phones[].value` and `emails[].value` from that file's front matter. See [contacts.md](contacts.md).

Compare emails trimmed, case-insensitive. Compare phones by digits only. A match is equal digits, or the longer number is the shorter one plus a single leading `1` when the shorter one has 10 digits. No other partial match. Several matches: use the filename that sorts first by raw UTF-8 bytes.

In the body, a matched person is `[[<title>]]`. With an email, write `[[Ada Lovelace]] <ada@example.com>`. In front matter, store that title as a plain string. The user's own unmatched sender stays `Me`.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The run finished. A window with nothing to write is success. |
| `1` | Bad arguments, corrupt state, missing OAuth client, clock went backward, or the lock exists. Nothing was written. |
| `2` | A category failed. Collect cursors are unchanged. No run folder is left when the failure was a fetch. A write failure can leave that run's folder, or a partly updated vault file, in place. |

Collect stdout begins with the run folder path when one was created. Then one line of `<category>=<files>` for each selected category. A dated run prints that line once per date that received a file, prefixed with `YYYY-MM-DD`. An empty success prints the zeros and no folder path.

Import stdout is `contacts=<files>`, the number of files created or updated.

## Worked example

Timezone `America/Los_Angeles`. No collect cursors. At 10:00 on 2026-09-27:

```text
pkmagent collect --since 2026-09-26
```

The window is `2026-09-26T00:00:00-07:00` inclusive through `2026-09-27T10:00:00-07:00` exclusive. That is two civil dates, so the folder is flat: `inbox/2026-09-27T100000-0700/` with one directory per category that had items. A thread with mail on both days becomes one Gmail file containing both messages. A message at 11:00 is not included. A dentist appointment on 2026-09-30 is not included. Each category's cursor becomes `2026-09-27T10:00:00-07:00`.

At 18:00, `pkmagent collect gmail` covers 10:00 inclusive through 18:00 exclusive and writes `inbox/2026-09-27T180000-0700/gmail/`. The 11:00 message is a new file there. Only the gmail cursor moves. The 10:00 folder is not opened.

The morning command with `--since 2026-09-25` covers three civil dates, so each file is under a date directory inside the run folder. The thread's Monday mail and Tuesday mail are two files.

`pkmagent import contacts` refreshes `<vault>/contacts` and does not create an inbox folder.

## Out of scope

- Sources other than Gmail, the local Messages database, EventKit, and the Contacts framework.
- The Google Calendar REST API.
- More than one Gmail account.
- A separate database of collected ids. The collect cursors are the record.
- Resuming or deleting a run folder left behind by a crashed collect.
