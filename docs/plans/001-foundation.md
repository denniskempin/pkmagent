# 001 — Foundation

Shared contract for `pkmagent`. Category plans ([002-gmail.md](002-gmail.md), [003-messages.md](003-messages.md), [004-calendar.md](004-calendar.md), [005-contacts.md](005-contacts.md)) implement sources against this file. They do not redefine the CLI, the lock, cursors, civil-date slicing, filename sanitizing, wiki-link matching, progress lines, or the collect write phase.

Behavior that is not source-specific is specified in [../specs/overview.md](../specs/overview.md). This plan says how to build it.

## Language

Rust. One crate, macOS-only at runtime, edition 2021.

Python was the alternative. It loses on this tool:

- macOS TCC binds Full Disk Access, Calendar, and Contacts to a Mach-O path. A Python interpreter path changes across venvs and Homebrew upgrades, so the grants do not stay attached to `pkmagent`.
- The program is a deterministic importer and a CLI. Python's agent libraries do not apply.
- PyObjC has more published EventKit and Contacts examples, and `markdownify` plus `google-api-python-client` are easier for Gmail. `NSUnarchiver` is available from both languages. Those advantages do not outweigh a single signed binary that already links Foundation.

Do not split formatting or HTTP into Python.

`cargo test` of the pure modules must pass on Linux. The EventKit, Contacts, and `chat.db` calls are behind `cfg(target_os = "macos")`. The source types themselves exist on every target. On other targets, `fetch` and the contacts snapshot return a category error (`<source> is only available on macOS`) so `src/main.rs` still builds. Fixture parsers and renderers are not cfg-gated.

## Crate

```text
src/
  main.rs
  cli.rs
  vault.rs
  timeutil.rs
  filename.rs
  note.rs
  contacts_index.rs
  progress.rs
  collect.rs
  sources/mod.rs
  sources/gmail.rs
  sources/messages.rs
  sources/calendar.rs
  sources/contacts.rs
```

Shared dependencies: `clap`, `serde`, `serde_json`, `chrono`, `chrono-tz`. Front matter is hand-written. Do not use `serde_yaml`; it reorders keys. Category plans add their own dependencies (HTTP, SQLite, Objective-C). They keep `crate::sources::<category>` as the module path.

`src/main.rs` maps a typed outcome to the process status and prints stdout and stderr. Library code returns that outcome. It does not call `process::exit`.

The Cargo package is a binary. Tests are `#[cfg(test)]` inside the modules they cover. Do not add `src/lib.rs`.

## Outcome

| Code | Variant | When |
| --- | --- | --- |
| `0` | success | The run finished, including a window with nothing to write. |
| `1` | rejected | Bad arguments, corrupt `state.json`, missing OAuth client, frozen end earlier than a selected cursor, or the lock file already exists. Nothing was written. |
| `2` | failed | A category failed. Collect cursors are unchanged. A fetch failure leaves no run folder. A write failure may leave that run's folder, or a partly updated contact file. |

Stderr for a collect failure, whether or not stderr is a terminal:

```text
YYYY-MM-DD failed <category>: <reason>
```

Import failure:

```text
contacts failed: <reason>
```

`<reason>` must not contain tokens, client secrets, or message bodies.

## Commands

`clap` in `src/cli.rs`. `--inbox PATH` is optional on every command. The default inbox is `./inbox`. The vault is the parent of the inbox path.

Collect categories are `gmail`, `messages`, `calendar`, in that order. Import's only category is `contacts`. A command with no category runs every category that command allows. Typed order is ignored. An unknown name, a name the command does not allow, or the same name twice is rejected (exit `1`) before the lock is taken.

| Command | Allowed | Rejected |
| --- | --- | --- |
| `pkmagent auth gmail` | that one subcommand | any category argument |
| `pkmagent import` | same as `import contacts` | `gmail`, `messages`, `calendar`, duplicates |
| `pkmagent collect` | zero or more of the three collect categories | `contacts`, unknown names, duplicates |
| `--since YYYY-MM-DD` | `collect` only | combined with `--day`; also rejected when every selected category already has a cursor |
| `--day YYYY-MM-DD` | `collect` only | combined with `--since`; `import` accepts neither flag |

`auth` does not take the lock and does not read or write cursors. The OAuth flow itself is [002-gmail.md](002-gmail.md). Foundation only checks that `<vault>/.pkmagent/secrets/gmail-client.json` exists before calling into Gmail; if it is missing, exit `1`.

`import` has no cursor. It does not read or write `state.json`.

## Vault

`src/vault.rs`.

| Path | Rule |
| --- | --- |
| `<vault>/.pkmagent/state.json` | Collect cursors. |
| `<vault>/.pkmagent/pkmagent.lock` | Exclusive lock for `import` and `collect`. |
| `<vault>/.pkmagent/secrets/` | Mode `0700`. |
| `<vault>/.pkmagent/secrets/gmail-client.json` | User-supplied. Keys `client_id`, `client_secret`. |
| `<vault>/.pkmagent/secrets/gmail.json` | Refresh token. Mode `0600`. |
| `<vault>/contacts/` | Import writes here. Collect only reads it. |
| `<inbox>/<run>/` | Collect creates this only when it has a file to write. |

Create a directory when about to write there. Do not create `contacts/` during collect. Do not create an inbox folder during import.

### Lock

`import` and `collect` only. Create `<vault>/.pkmagent/pkmagent.lock` with `create_new` so two processes cannot both pass the check. If the file exists, exit `1` and tell the user to delete it when nothing is running. Write the pid as text. Remove the file on every return path, including a category failure. A killed process can leave it. There is no `--force` and no `flock`.

Take the lock only after argument parsing succeeds. A bad argument must not create `.pkmagent/`.

### state.json

```json
{
  "gmail": "2026-09-27T18:04:11-07:00",
  "messages": null,
  "calendar": null
}
```

A missing file, a missing key, or `null` means that category has never been collected. Each present value is RFC3339 with a numeric offset and no fractional seconds. `Z` is corrupt. Any other key, any other JSON type, or a non-object root is corrupt: take no lock writes beyond removing the lock, write no inbox files, exit `1`.

Note files are written directly, with no temporary name. `state.json` is the exception: write a sibling temp file and rename it over the destination so a crash cannot leave a half-written cursor file. On success of a cursor-moving collect, set each selected category's cursor to `window_end` and leave the other keys as they were. `--day` does not change the file. `import` and `auth` do not change the file.

Replacing `gmail.json` uses the same temp-and-rename pattern. That file is a secret, not a note. The Gmail plan owns its contents.

## Clock and windows

`src/timeutil.rs`. Tests inject a clock. Production uses the machine's local offset and IANA name.

```rust
pub struct Slice {
    pub start: DateTime<FixedOffset>, // inclusive
    pub end: DateTime<FixedOffset>,   // exclusive
    pub day: NaiveDate,
}
```

Freeze `window_end` before any fetch. Floor it to the whole second. Items in the remainder of that second wait for the next run.

For each selected category:

- No cursor: `--since` is required. `window_start` is local midnight at the start of that date. `--since` does not move a category that already has a cursor.
- Has a cursor: `window_start` is that cursor.
- A `--since` midnight after the frozen end is rejected.
- If every selected category has a cursor, `--since` is rejected.
- If the frozen end is earlier than any selected cursor, exit `1` and write nothing.
- A cursor equal to the frozen end contributes no items and is still success. Its cursor is set to `window_end` again on a normal collect.

Include items with `window_start <= t < window_end`. Each source defines `t`.

The run's civil-date list runs from the date of the earliest selected `window_start` through the date of the last instant inside the window. Step with the timezone's calendar, not by adding 24 hours, so a DST transition neither skips nor repeats a date. An empty window (start equal to end) visits no dates. `--day` visits that one date, midnight inclusive through the next midnight exclusive, and ignores cursors for the slice. It still refuses a frozen end that is earlier than a selected cursor only for the non-`--day` path; `--day` does not consult cursors.

For civil date `D` and a category whose own start is `S`:

- `slice_start` is the later of `S` and local midnight at the start of `D`.
- `slice_end` is the earlier of `window_end` and local midnight at the start of `D+1`.
- If `slice_start >= slice_end`, the job succeeds with no items and does not call the source.

`--day` uses the whole civil day as the slice for every selected category.

The run folder name is the frozen end in the local offset, colons removed: `2026-09-27T100000-0700`. If that directory exists, append `-2`, `-3`, and so on. Choose the suffix at write time. A failed fetch must not create the directory.

### Worked example the tests lock in

Zone `America/Los_Angeles`. No cursors. Now is `2026-09-27T10:00:00-07:00`.

`pkmagent collect --since 2026-09-26` has window `2026-09-26T00:00:00-07:00` inclusive through `2026-09-27T10:00:00-07:00` exclusive. Two civil dates, so the layout is flat. Each cursor becomes `2026-09-27T10:00:00-07:00`.

A later `pkmagent collect gmail` at `2026-09-27T18:00:00-07:00` covers gmail from 10:00 inclusive through 18:00 exclusive and moves only the gmail cursor. The 10:00 folder is not opened.

The same morning command with `--since 2026-09-25` covers three civil dates, so the layout is dated.

Also test `America/Los_Angeles` across `2026-03-08` (spring forward) and `2026-11-01` (fall back): each civil date appears once, and the fall-back slice is local midnight to the next local midnight.

## Source trait

`src/sources/mod.rs`. Collect sources return owned items. Live backends sit behind the trait. Tests use fakes.

```rust
pub trait CollectSource {
    fn fetch(
        &self,
        slice: &Slice,
        contacts: &ContactIndex,
    ) -> Result<Vec<CollectedItem>, SourceError>;

    fn group_extra_front_matter(&self, items: &[CollectedItem]) -> String {
        items.first().map(|item| item.extra_front_matter.clone()).unwrap_or_default()
    }
}

pub struct CollectedItem {
    pub stable_id: String,
    pub display_title: String,
    pub empty_title_fallback: String,
    pub source_url: String,
    pub t: DateTime<FixedOffset>,
    pub tie_break: String,
    pub file_day: NaiveDate,
    pub extra_front_matter: String,
    pub body: String,
    pub attachments: Vec<Attachment>,
}

pub struct Attachment {
    pub filename: Option<String>,
    pub media_type: Option<String>,
}
```

- `t` is the window instant from the source spec. Foundation drops items outside the slice even if a source returns them.
- `tie_break` orders equal `t` values, ascending raw UTF-8. Gmail uses the message id. Messages uses the database id. A single calendar occurrence uses an empty string.
- `file_day` is the civil date the item is filed under. It is the local date of `t` unless the source spec says otherwise (all-day calendar events).
- `empty_title_fallback` is the source spec's fallback (`untitled`, `(no subject)`, or the Messages fallbacks). Filename sanitizing uses it when the title is empty.
- `extra_front_matter` is YAML, already rendered, to be inserted after the shared keys. Omit optional fields with no value. No document markers. Foundation does not reorder it.
- `body` is the markdown after the source link. Foundation writes the link, then this body, then attachments. The body does not repeat the title as a heading.
- Sources call `ContactIndex` while building `extra_front_matter` and `body`. Foundation loads the index once, before any fetch, from `<vault>/contacts`. A missing directory is an empty index, not an error. Sources then leave addresses as they wrote them.

`ContactIndex` (`src/contacts_index.rs`):

- Consider only markdown files whose front matter has `id`. The contact title is the filename without `.md`.
- `title_for_email`: trim both sides, compare case-insensitively.
- `title_for_phone`: digits only. A match is equal digits, or the longer number is the shorter one plus a single leading `1` when the shorter one has 10 digits. No other partial match.
- Several matches: the filename that sorts first by raw UTF-8 bytes.
- In the body, a matched person is `[[<title>]]`. With an email, `[[Ada Lovelace]] <ada@example.com>`. In front matter, the title is a plain string. The user's own unmatched sender stays `Me`.

Category plans say where the link is placed. They do not restate these comparison rules.

Import is not a `CollectSource`. `src/sources/contacts.rs` exposes a snapshot method that `import` calls. The record fields and the create-versus-refresh rules are [005-contacts.md](005-contacts.md).

## Collect run

`src/collect.rs`.

A job is one category fetch for one civil date on the run's date list. At most one fetch per category runs at a time. The next date for a category starts after that category's earlier date finishes. Categories run concurrently with each other.

If a job fails, start no further jobs and discard in-flight results. Write no run folder. Leave every cursor unchanged. Exit `2`.

After every fetch has succeeded:

1. Group items into output directories.
2. If there is nothing to write, print the zero lines and do not create a folder. On a normal collect, still move each selected cursor to `window_end`. `--day` does not.
3. Otherwise create the run folder and write files directly.
4. If a write fails, leave every cursor unchanged and exit `2`. The folder may already contain files. The next collect does not open it.
5. On success, move cursors unless this is `--day`.

Layout, using the run's civil-date list (not each category's own shorter window):

- One or two dates: `<run>/<category>/`. A thread or chat is one file for the whole run. Every event occurrence goes in `<run>/calendar/`.
- More than two dates: `<run>/<YYYY-MM-DD>/<category>/`. A thread or chat is one file per date. An event occurrence goes under its `file_day`.
- A category with nothing to write gets no directory. A date with nothing to write gets no date directory.

Within a directory, group by `stable_id`. Sort items by `t`, then `tie_break`. One file. Collect only creates files. It never reads or edits an older inbox folder.

A sorted group becomes that file as follows. Sources return one item per message or occurrence; they do not pre-merge.

- The filename uses the last item's `display_title` and `empty_title_fallback`. Messages puts the same title on every item, so the last item is the chat title. Gmail puts each message's subject on that item, so the last item is the chronologically last subject.
- `source_url` is the first item's `source_url`. A thread or chat stores the same URL on every item.
- `day` is the civil date of the earliest item.
- `extra_front_matter` is `group_extra_front_matter` on the source. The default returns the first item's block. Gmail overrides it so a flat two-day thread unions participants from every fetch. Messages leaves the default, because every item in the chat carries the same YAML.
- The body is the source link, a blank line, then each item `body` in order. Each item body has no leading newline and one trailing newline. One extra newline between items makes a blank line between sections.
- Attachments are one list: each item's attachments, in group order. `None` filename renders as `unnamed`. `None` media type is omitted.

### Stdout

When a run folder was created, its path is the first line, relative to the process working directory when the inbox is relative.

Then one line `<category>=<files>` for each selected category, in category order. A dated run prints that line once per date that received a file for that category, prefixed with the date:

```text
inbox/2026-09-25T100000-0700
2026-09-25 gmail=1
2026-09-27 calendar=1
```

A flat run has no date prefix. An empty success prints the zeros and no folder path:

```text
gmail=0
messages=0
calendar=0
```

`<files>` counts files, not items inside a file.

Import stdout is `contacts=<files>`, the number of files created or updated. Deletions are not included. Import creates no inbox folder.

## Filenames

`src/filename.rs`. Choose a name only when creating a file. Import never renames.

`title` in front matter is the filename without `.md`, including a collision suffix.

Sanitizing the display title:

1. Collapse whitespace to a single space and trim.
2. If empty, use `empty_title_fallback`.
3. Replace `\ / : * ? " < > |` and ASCII controls with `-`.
4. Trim trailing spaces and dots.
5. If the result is empty, `.`, or `..`, use `untitled`.
6. Truncate to 80 Unicode scalar values.
7. If the UTF-8 filename including `.md` would exceed 200 bytes after a suffix, truncate the title further until it fits.

Create files in a directory in ascending raw UTF-8 order of `stable_id`. Prefer `Title.md`. If that name is taken, including a case-insensitive match among names already chosen in that directory, use `Title--<suffix>.md`. The suffix is `stable_id` sanitized the same way, truncated to 40 Unicode scalar values. If that name is taken, append `-2`, `-3`, and so on.

## Note format

`src/note.rs`. UTF-8, LF, one trailing newline. A new file is deterministic except for `imported_at`.

Shared front matter, in this order:

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

Quote `id`, `title`, and `source_url`. `source` is `gmail`, `messages`, `calendar`, or `contacts`. `imported_at` is the frozen run time as RFC3339 with a numeric offset, and it is written only when the file is created. On a collect file, `day` is the civil date of the earliest item in the file. In a dated run that is the date directory. `timezone` is the machine IANA zone. Contact files omit `day` and `timezone`.

Insert `extra_front_matter` after these keys. Then the closing `---`.

The body starts with the source link. Link text is `Open in Gmail`, `Open in Messages`, `Open in Calendar`, or `Open in Contacts`. The URL is `source_url`.

Attachments, when present, follow the body under `### Attachments`. Each item is `filename (media type)`. A missing filename is `unnamed`. Omit the media type when it is unknown. Do not download attachment bytes.

## Progress

`src/progress.rs`. When stderr is a terminal, rewrite one status line per selected category, in category order. When it is not, print nothing. Failure lines from the outcome section are always printed.

Each line is the category padded to 8 columns, `<done>/<total>`, a 20-column bar, and a label:

```text
gmail    2/3 [=============>      ] 2026-09-27
messages 1/3 [======>             ] 2026-09-26
calendar 0/3 [                    ] waiting
```

`<total>` is the number of civil dates in the run list, or `1` for `--day`. Before a category starts, the label is `waiting` and the bar is 20 spaces. While a date is in flight or was the last one finished, the label is that date. The bar fills with `=` and a trailing `>` until complete, then 20 `=` characters. Redraw by moving the cursor up; do not scroll a new block per update.

`import` shows one line. `<total>` is the number of people in the snapshot. The label is the display title most recently written, or `waiting`.

## Tests

Pure tests, no TCC prompt, no live `chat.db`:

- Reject `import gmail`, `import messages`, `import calendar`, `collect contacts`, duplicate names, unknown names, `--since` with `--day`, `--since` when every selected category has a cursor, and a `--since` midnight after the frozen end.
- `collect calendar gmail` still runs gmail first.
- Corrupt `state.json` (extra key, `Z`, fractional seconds, array root) exits `1` and creates no inbox file.
- Missing `state.json` means all cursors are unset.
- The worked example, the three-date layout, an empty category directory omitted, an empty success, cursor equal to the frozen end, and the clock-went-backward rejection.
- DST dates `2026-03-08` and `2026-11-01` in `America/Los_Angeles`.
- Filename collisions, case-insensitive matches, the 80-scalar cap, and the 200-byte cap.
- Front matter key order, contact files omitting `day` and `timezone`, and `imported_at` absent from a refreshed contact body.
- Email and phone matching, including the leading-`1` rule and the UTF-8 filename tie-break.
- Fetch failure: no folder, cursors unchanged, exit `2`, the dated failure line on stderr.
- Write failure: cursors unchanged, exit `2`, partial folder allowed.
- `--day` writes a folder and does not change cursors.
- Lock exists: exit `1`, and the lock is removed after a normal failure.
- Progress formatter matches the three-line example when stderr is not consulted; a non-terminal sink records no progress text.

Fake `CollectSource` values drive the orchestrator. Category fixtures live under `tests/fixtures/` and are named in the category plans.

## Runtime note

Grant Full Disk Access, Calendar, and Contacts to a release binary at a stable path. `cargo run` changes the binary identity, so the grants do not follow it. The first live check, after the category work exists, is: `collect` with no category writes Gmail, Messages, and Calendar and does not write `<vault>/contacts`; `import` updates contacts and creates no inbox folder; `import gmail` and `collect contacts` are rejected.

## Out of scope

- Sources other than Gmail, the local Messages database, EventKit, and the Contacts framework.
- The Google Calendar REST API.
- More than one Gmail account.
- A database of collected ids. The cursors are the record.
- Resuming or deleting a run folder left by a crashed collect.
- `--force`.
