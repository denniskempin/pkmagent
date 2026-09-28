# 003 — Messages

Implement `crate::sources::messages` in `src/sources/messages.rs`. Behavior that is not specific to this source is already fixed by [001-foundation.md](001-foundation.md) and [../specs/overview.md](../specs/overview.md). Do not redefine the CLI, the lock, cursors, civil-date slicing, filename sanitizing, contact matching, progress, or the collect write phase.

`CollectSource::fetch(slice, contacts) -> Result<Vec<CollectedItem>, SourceError>`. One `CollectedItem` per included message. Foundation groups by `stable_id` inside a directory and sorts by `t`, then `tie_break`.

The live open of `~/Library/Messages/chat.db` is `cfg(target_os = "macos")`. The SQL, row mapping, titles, links, and section markdown run on Linux against a fixture database. `rusqlite` with `bundled` is the SQLite on both platforms.

## Module shape

```text
src/sources/messages.rs
tests/fixtures/messages/schema.sql
tests/fixtures/messages/seed.sql
```

Public surface:

- `MessagesSource` implements `CollectSource`. On macOS, `fetch` opens the live database and calls `fetch_messages`. On any other target, `fetch` returns the reason `Messages is only available on macOS` and does not touch a path.
- `pub fn fetch_messages(conn: &rusqlite::Connection, slice: &Slice, contacts: &ContactIndex, zone: &chrono_tz::Tz) -> Result<Vec<CollectedItem>, SourceError>` does the query and the mapping. Tests call this. It does not open `~/Library/Messages/chat.db`.
- `zone` is the IANA zone foundation already resolved for the run (the value it writes as `timezone:`). Tests pass `America/Los_Angeles`. Convert each sent instant in that zone. Do not sample a second clock.

Each `fetch` opens the database, reads, and drops the connection. There is no process-long handle.

`tie_break` is the message `ROWID` rendered as a 20-digit zero-padded decimal (`format!("{rowid:020}")`). Foundation sorts `tie_break` as raw UTF-8. Equal width makes that order the same as numeric order (`"10"` sorts before `"9"` when the strings are unpadded). The section line `- Id:` uses the unpadded decimal.

`file_day` is the local civil date of `t` (`t.date_naive()` after the zone conversion). `stable_id` is `chat.guid` as stored. Every item in a chat carries the same `display_title`, `empty_title_fallback`, `source_url`, and `extra_front_matter`.

## Open the database

Resolve the path as `$HOME/Library/Messages/chat.db`. If `HOME` is unset, fail with `cannot open Messages database: home directory is unknown`.

Open with rusqlite `OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI` and this URI:

```text
file:<percent-encoded absolute path>?mode=ro
```

Encode each byte of the absolute path except `A-Z a-z 0-9 / - _ .`. Use uppercase hex (`%20` for a space). Do not set `immutable=1`. That flag skips the lock manager, so an open Messages.app still reads, and frames that exist only in the WAL can be missing. The spec requires a locked database to fail the category.

After a successful open, run `PRAGMA query_only = ON` and `PRAGMA busy_timeout = 0`. `query_only` refuses writes. `busy_timeout = 0` makes a contended lock return immediately instead of waiting and then succeeding. A read still sees the WAL when the shared lock can be taken.

Do not open `chat.db-wal` or `chat.db-shm` as the database. Do not read attachment files under `~/Library/Messages/Attachments`.

Full Disk Access is required. Without it, `open` fails with `EPERM` / `Operation not permitted`. Classify the rusqlite error and return one of these reason strings. Foundation prints `YYYY-MM-DD failed messages: <reason>`. The reason is the SQLite code and `errmsg` only. Never append `text`, `attributedBody`, a handle, or any other column value.

| Condition | Reason |
| --- | --- |
| `SQLITE_BUSY` or `SQLITE_LOCKED` | `Messages database is locked` |
| `SQLITE_CANTOPEN` or `SQLITE_AUTH`, and the OS error is `EPERM` or `EACCES`, or `errmsg` contains `operation not permitted` or `permission denied` (ASCII case-insensitive) | `Messages database is not readable (Full Disk Access required)` |
| Any other open failure, including a missing file | `cannot open Messages database: <errmsg>` |
| A later query or pragma failure | `cannot read Messages database: <errmsg>` |

A missing file uses the third row, not the Full Disk Access sentence.

`SQLITE_BUSY` or `SQLITE_LOCKED` from the open or from any later statement fails the category with `Messages database is locked`. That includes Messages.app holding a lock this process cannot share. Do not retry.

## Tables and columns

Read these tables. Select these columns. Extra columns exist on a real `chat.db`; ignore them. `date_retracted` and `associated_message_emoji` arrived in later macOS releases. If `PRAGMA table_info` says a column is missing, treat `date_retracted` as `0` and `associated_message_emoji` as null and continue. Any other missing column fails the category with `cannot read Messages database: <errmsg>`.

`message`

| Column | Use |
| --- | --- |
| `ROWID` | Section id and `tie_break` |
| `guid` | Reaction target lookup |
| `text` | Plain body when non-empty |
| `attributedBody` | Blob used only when `text` is null or `""` |
| `handle_id` | Join to the sender handle |
| `is_from_me` | Non-zero means the user sent it |
| `date` | Sent time. This is `t` |
| `is_sent` | Outgoing rows with `0` are omitted |
| `cache_has_attachments` | Selected and ignored. The join is authoritative |
| `associated_message_type` | Tapback kind |
| `associated_message_guid` | Target message |
| `associated_message_emoji` | Custom reaction name when the column exists |
| `destination_caller_id` | User's own handle for that row |
| `account` | Fallback self handle, often `E:<email>` or `P:<phone>` |
| `date_retracted` | Non-zero means the message was unsent |
| `item_type` | Non-zero is a group-action balloon; omit it |
| `date_read` | Loaded in the fixture only so tests can show it is not `t`. Do not select it for production decisions |

`handle`: `ROWID`, `id`. Store `id` as the handle. Do not rewrite it.

`chat`: `ROWID`, `guid`, `style`, `display_name`, `last_addressed_handle`, `account_login`. Style `45` is a direct chat. Style `43` is a group. `display_name` is the Messages name of that chat. `room_name` is not a title.

`chat_message_join`: `chat_id`, `message_id`.

`chat_handle_join`: `chat_id`, `handle_id`. This is the full membership. A person who sent nothing in the slice is still a participant.

`attachment`: `ROWID`, `filename`, `mime_type`, `transfer_name`, `is_sticker`.

`message_attachment_join`: `message_id`, `attachment_id`.

A message joined to two chats becomes one item per chat. `stable_id` differs, so they are different files.

## Sent time

`message.date` is an Apple absolute time since `2001-01-01 00:00:00 UTC`. The Unix offset of that epoch is `978307200`.

Cutoff: **strictly greater than `1_000_000_000_000_000` (`10^15`) means nanoseconds; otherwise seconds.** Seconds through year 33658 stay below the cutoff. The nanosecond format began in 2017, and those values are about `10^17`, above the cutoff. A value equal to the cutoff is seconds.

```text
secs, subsec = if date > 10^15 then date.div_euclid(10^9), date.rem_euclid(10^9)
               else date, 0
utc = secs + 978307200, plus subsec nanoseconds
t   = that instant in `zone`, stored as DateTime<FixedOffset>
```

Keep the nanosecond remainder on `t` so the slice comparison is exact. Format headings with whole seconds, `format("%Y-%m-%d %H:%M:%S %z")`, which prints `2026-09-27 09:01:00 -0700`.

Skip `date = 0`. Skip a value `DateTime::from_timestamp` rejects. Skipping a row is not a category failure.

SQL is a prefilter. Bind both representations of the slice, using `slice.start.timestamp()` and `slice.end.timestamp()` minus `978307200`:

```sql
(m.date > 1000000000000000 AND m.date >= :nano_start AND m.date < :nano_end)
OR
(m.date <= 1000000000000000 AND m.date >= :sec_start AND m.date < :sec_end)
```

Then keep a row only when `slice.start <= t < slice.end`. Foundation drops out-of-slice items as well. `date_read` and `date_delivered` never enter this comparison.

Multiply the second bounds by `1_000_000_000` with saturating arithmetic so a far-future slice does not panic. A cutoff of `10^9` is too low: a seconds value in 2033 is already past it and would be read as nanoseconds. `10^15` stays a seconds value until year 33658, and every nanosecond timestamp from the 2017 format is above it.

Worked values for `America/Los_Angeles` (`-07:00` on these dates):

| Local sent time | Seconds | Nanoseconds |
| --- | --- | --- |
| 2026-09-26 12:00:00 | 812142000 | 812142000000000000 |
| 2026-09-27 09:01:00 | 812217660 | 812217660000000000 |
| 2026-09-27 21:30:00 | 812262600 | 812262600000000000 |
| 2026-09-28 00:30:00 | 812273400 | 812273400000000000 |

## Which rows become items

Omit a row when any of these hold:

- `is_from_me != 0` and `is_sent = 0` (composed, never sent). Incoming rows stay, whatever `is_sent` is.
- `date_retracted` is not null and not `0` (the user unsent it).
- `item_type` is not null and not `0`.
- `associated_message_type` is in `3000..4000` (a tapback removal).
- The row is not a reaction, not a sticker, and has no plain text and no attachment.

Include a row that survives that filter:

1. **Reaction**, when `associated_message_type` is `2000`–`2005`, or `associated_message_emoji` is non-empty after trimming Unicode whitespace. Standard types use the stored name, even if an emoji is also set:

   | `associated_message_type` | Name |
   | --- | --- |
   | 2000 | Love |
   | 2001 | Like |
   | 2002 | Dislike |
   | 2003 | Laugh |
   | 2004 | Emphasize |
   | 2005 | Question |

   Any other type with a non-empty emoji uses the trimmed emoji as the custom name. A type outside that table with an empty emoji is not a reaction. Do not omit it here. Continue with the sticker and message rules. A normal text row has type `0` or null and no emoji, so it is a message.

2. **Sticker**, when some joined attachment has `is_sticker != 0`. A reaction that also has a sticker attachment stays a reaction.

3. **Message**, otherwise. This covers text and attachment-only rows, including rows the user sent (`is_from_me != 0` and `is_sent != 0`) and rows in group chats.

Group chats and direct chats use the same row rules.

## Plain text

`plain_text(text, attributed_body) -> Option<String>`:

- When `text` is `Some` and not `""`, return it. Do this even if `attributedBody` is also set. Whitespace-only `text` is non-empty and is kept.
- When `text` is null or `""` and the body blob is non-empty, unarchive it on macOS (next section). On any other target, return `None`. Do not fail the category.
- Normalize `\r\n` and bare `\r` to `\n` in the returned string.

Message section bodies keep that string. Reaction quotes pass it through whitespace collapsing. Do not escape Markdown.

## attributedBody

On current macOS, `text` is often null. `attributedBody` is an `NSArchiver` typedstream of an `NSAttributedString`. The blob starts with `04 0b` and the ASCII word `streamtyped`. It is not an `NSKeyedArchiver` archive.

**Approach: `NSUnarchiver` via `objc2`.** This function is `cfg(target_os = "macos")` only.

- Depend on `objc2` and `objc2-foundation` only under `[target.'cfg(target_os = "macos")'.dependencies]`.
- Inside an autorelease pool, build `NSData` from the blob and send `+unarchiveObjectWithData:` to the class `NSUnarchiver`. Send `-string` to the result and copy the UTF-8. `NSKeyedUnarchiver` does not read this format.
- A null result, a failed send, or a missing `string` yields `None` for that message. The category still succeeds. The blob bytes never appear in a reason string or in the note.

Do not add a typedstream parser and do not depend on the `typedstream` crate. `NSUnarchiver` is part of Foundation on the macOS runtime, which is the only place a live `chat.db` is opened. The Linux test build does not link Objective-C. Fixture rows store the body in `text`, so the suite never needs a decoder. A parser would be untested against real blobs.

## Self handles

Self is a property of the chat, taken from every message in that chat, not only the slice. First source that yields a handle wins. Later sources are not consulted.

1. Distinct `destination_caller_id` values that are non-empty after trimming ASCII whitespace.
2. Else distinct `account` values, with one leading `E:` or `P:` removed when that prefix is present. Drop empties.
3. Else `chat.last_addressed_handle` when non-empty.
4. Else `chat.account_login` when non-empty.

Compare self to a member with exact string equality. `+15551212` and `15551212` are different handles. Do not use `ContactIndex` to decide self.

Membership is `handle.id` from `chat_handle_join`. Add self handles that are not already members (the user is usually absent from that join). If a member's exact string is a self handle, that one entry is `self: true`. Dedupe by exact handle before sorting.

## Display title

Resolve a handle to a contact with `ContactIndex`. When the handle contains `@`, call `title_for_email`. Otherwise call `title_for_phone`. Matching itself stays in `ContactIndex`.

The Messages display name of a **direct** chat is `chat.display_name` after trimming Unicode whitespace, when that leaves a non-empty string. There is no per-handle name column on `handle`. Group members therefore have no Messages display name.

Other participants are members that are not self.

| Chat | `display_title` | `empty_title_fallback` |
| --- | --- | --- |
| No other participants | `Me` | `Me` |
| Direct: style `45`, or any other style, with exactly one other participant | Contact title, else the direct chat's Messages display name, else the handle as stored | That handle as stored |
| Group: style `43`, or more than one other participant | `display_name` after trimming Unicode whitespace, when that is non-empty. Otherwise the joined labels | The joined labels. If there are none, `Me` |

A whitespace-only `display_name` (`'   '`) is no name. Trim the ends of a real name and keep its internal spaces (`"  Book club  "` is `Book club`).

A joined label is, in order, the contact title, else the handle as stored. Self is not a label. Sort the labels by raw UTF-8 bytes and join them with `", "` (comma, space). When two labels are equal, the handle's raw UTF-8 order breaks the tie.

`Ada Lovelace` sorts before `ada@icloud.com` because `A` is byte `0x41` and `a` is byte `0x61`.

A style `45` chat with two other participants uses the group rule, so the second person is not dropped. A style `43` chat with one other participant and no name uses the group rule; its joined title is that one label.

The outgoing sender in a section is the literal `Me`, including when the user's handle matches a contact.

## Participants and `source_url`

`extra_front_matter` is hand-written YAML, no `serde_yaml`, no document markers. One trailing newline. Always emit `participants`.

Sort participants by handle, raw UTF-8. For each entry, write `name` only when it is non-empty, then `handle`, then `self: true` only for a self handle. Omit `self` on everyone else. `name` is the contact title when the handle matches. On a direct chat, if the contact does not match, `name` is that chat's Messages display name. Omit `name` when both are empty. Do not put the raw handle in `name`. Quote `name` and `handle` with YAML double quotes, escaping `\`, `"`, and bytes below `0x20`.

```yaml
participants:
  - name: "Ada Lovelace"
    handle: "+15551212"
  - handle: "ada@icloud.com"
  - handle: "me@icloud.com"
    self: true
```

An empty membership with no self handle is `participants: []`.

`source_url` uses the other participants of the **full chat**, in this front-matter order, including people who sent nothing in the slice.

1. Scheme `sms` when `chat.guid` starts with the exact bytes `SMS;`. Otherwise `imessage`.
2. Take every participant that is not self, in that sorted order.
3. Percent-encode each handle as UTF-8. Leave bytes in `A-Z a-z 0-9 - . _ ~` as themselves. Encode every other byte as uppercase `%HH`. That encodes `+` as `%2B` and `@` as `%40`. A comma inside a handle becomes `%2C`.
4. If none remain, `source_url` is `messages://`.
5. Otherwise join the encoded handles with a raw comma: `<scheme>://<handle>,<handle>,...`.

Examples: `imessage://%2B15551212`, `imessage://%2B15551212,ada%40icloud.com`, `sms://%2B15559876`, `messages://`.

## Message sections

`body` is the markdown after the source link. Do not write the chat title, `Open in Messages`, or `### Attachments`. Foundation writes the link and the attachment list.

Each body is one section and ends with a single newline.

Text:

```markdown
## 2026-09-27 09:01:00 -0700 — [[Ada Lovelace]]

- Id: 100

See you there.
```

The dash in the heading is U+2014, with a space on each side. The sender is `Me` when `is_from_me != 0`. Otherwise it is `[[<contact title>]]` when the sender handle matches, else the direct chat's Messages display name when this is a direct chat and that name exists, else the handle as stored. Do not append the handle after a wiki link. An incoming row whose `handle_id` does not resolve uses the literal `unknown`.

`- Id:` is the unpadded `ROWID`. Then a blank line, then the plain text. An attachment-only section ends after the Id line.

Reaction, one line, no extra text from the reaction row:

```text
Reaction: Like to "dinner at 7"
```

The quote is the target message's plain text with Unicode whitespace collapsed (`split_whitespace`, joined by one ASCII space), truncated to 80 Unicode scalar values, with no ellipsis. The target may sit outside the slice. `associated_message_guid` is `p:0/<message.guid>` or `bp:<message.guid>` or a bare guid. If the value contains `/`, the guid is the substring after the last `/`. Else if it contains `:`, the guid is the substring after the last `:`. Else it is the whole string. Look up `message.guid` in the whole database.

When the collapsed text is empty, the quote is the target's first attachment display filename. First means lowest `attachment.ROWID`. When that is missing, the quote is `a message`. Wrap the quote in `"` and leave `"` inside the quote as they are. The collapsed quote has no newline.

Sticker:

```text
Sticker: heart.png
```

The filename is the sticker attachment's display name. When it has none, the line is `Sticker: sticker`.

Display filename, for sticker lines, reaction quotes, and `Attachment` metadata: trimmed `transfer_name` when non-empty, else the last `/`-separated component of `filename`. Empty means no filename.

## Attachments

Fill `CollectedItem.attachments` with metadata for every joined attachment, stickers included, ordered by `attachment.ROWID` ascending. Do not read bytes from disk.

Foundation's attachment record is a filename and a media type, either of which can be absent:

- Filename: the display filename, or absent (the renderer prints `unnamed`).
- Media type: `mime_type` when it is non-empty. Leave it absent when `mime_type` is null. Do not substitute `uti`.

`cache_has_attachments = 0` with a join row still produces an attachment.

## One file per chat

This source returns one item per message and does not pre-merge a chat. Foundation groups a directory by `stable_id`, sorts by `t` then `tie_break`, and joins the group with the rules in [001-foundation.md](001-foundation.md). `display_title`, `source_url`, `stable_id`, and `extra_front_matter` are identical on each item of a chat, so the default `group_extra_front_matter` is the file's YAML and the last item's title is the chat title. Do not override `group_extra_front_matter`.

## Dependencies

In `Cargo.toml`:

```toml
rusqlite = { version = "0.40", features = ["bundled"] }

[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.6"
objc2-foundation = { version = "0.3", features = ["NSString", "NSData", "NSObject"] }
```

`objc2` is only for the `NSUnarchiver` path. Linux `cargo test` must not need an Objective-C runtime or a system `libsqlite`.

## Fixtures

`tests/fixtures/messages/schema.sql` creates the tables and columns above. `tests/fixtures/messages/seed.sql` loads the rows below. Tests apply both to a temporary database with `rusqlite`. No checked-in binary `chat.db`, and no test reads a home-directory `chat.db`.

Contacts for these tests: one file `Ada Lovelace.md` in a temporary contacts directory, front matter `id` plus `phones[].value` of `+15551212`, loaded through `ContactIndex`. The contact title is the filename.

Zone `America/Los_Angeles`. Slice A is `2026-09-27T00:00:00-07:00` inclusive through `2026-09-28T00:00:00-07:00` exclusive. Slice B is the next local midnight through the midnight after that.

Handles: `1 = +15551212`, `2 = ada@icloud.com`, `3 = +15559876`, `4 = +15550000`.

Chats:

| Chat | guid | style | display_name | How self is stored |
| --- | --- | --- | --- | --- |
| 1 direct | `iMessage;-;+15551212` | 45 | null | `destination_caller_id = me@icloud.com` on every row, and `account = E:other@icloud.com`. Self must be `me@icloud.com` only |
| 2 unnamed group | `iMessage;+;chat555` | 43 | three spaces | `destination_caller_id = me@icloud.com` |
| 3 SMS | `SMS;-;+15559876` | 45 | null | `destination_caller_id = me@icloud.com` |
| 4 self only | `iMessage;-;self` | 45 | null | `destination_caller_id` null, `account = E:me@icloud.com`. No `chat_handle_join` rows |
| 5 renamed direct | `iMessage;-;+15550000` | 45 | `Sam` | `destination_caller_id = me@icloud.com` |
| 6 named group | `iMessage;+;chat777` | 43 | `Book club` | `destination_caller_id` null, `account` null, `last_addressed_handle = me@icloud.com` |

Membership: chat 1 has handle 1. Chat 2 has handles 1 and 2. Chat 3 has handle 3. Chat 4 has none. Chat 5 has handle 4. Chat 6 has handles 1 and 2.

Message times are local on 2026-09-27 unless noted. `is_sent` is `1` except row 108. `date_retracted` is `0` except row 109. `item_type` is `0`. `attributedBody` is null except row 100.

| ROWID | Chat | Sender | text | Apple `date` | Kind |
| --- | --- | --- | --- | --- | --- |
| 100 | 1 | handle 1 | `See you there.` | seconds `812217660` (09:01) | Text. `attributedBody` is the bytes `not-a-stream`. `date_read` is `812300000`, outside slice A. guid `G100` |
| 101 | 1 | from me | `On my way` | nanos `812217720000000000` (09:02) | Text, nanosecond magnitude |
| 102 | 1 | from me | null | seconds `812217840` (09:04) | Like (`2001`). Target `p:0/G115` |
| 103 | 1 | handle 1 | null | seconds `812217900` (09:05) | Attachment-only. `cache_has_attachments = 0`. guid `G103` |
| 104 | 1 | handle 1 | null | seconds `812217960` (09:06) | Sticker |
| 105 | 1 | handle 1 | null | seconds `812218020` (09:07) | Sticker with no filename |
| 106 | 1 | from me | null | seconds `812218080` (09:08) | Love (`2000`). Target `p:0/G103` |
| 107 | 1 | handle 1 | null | seconds `812218140` (09:09) | Laugh (`2003`). Target `p:0/does-not-exist` |
| 108 | 1 | from me | `secret unsent body` | seconds `812218200` (09:10) | `is_sent = 0`. Omitted |
| 109 | 1 | handle 1 | `secret retracted body` | seconds `812218260` (09:11) | `date_retracted` non-zero. Omitted |
| 110 | 1 | handle 1 | `too early` | seconds `812142000` (2026-09-26 12:00) | `date_read = 812217660` (inside slice A). Omitted |
| 111 | 1 | handle 1 | `Late` | seconds `812262600` (21:30) | Text. `file_day` 2026-09-27 |
| 112 | 1 | from me | null | seconds `812218320` (09:12) | Emphasize (`2004`). Target `p:0/G112` |
| 113 | 1 | handle 1 | null | seconds `812218680` (09:18) | Custom. `associated_message_type = 2006`, `associated_message_emoji = Cheer`. Target `p:0/G100` |
| 115 | 1 | handle 1 | `dinner \n\n at   7` | seconds `812142300` (2026-09-26 12:05) | Outside slice A. guid `G115`. Quote target only |
| 116 | 1 | handle 1 | 79 `a`, then U+00E9, then `Z` | seconds `812142360` (2026-09-26 12:06) | Outside slice A. guid `G112`. Quote target only |
| 200 | 3 | handle 3 | `Ping` | seconds `812218380` (09:13) | SMS |
| 300 | 4 | from me | `Note to self` | seconds `812218440` (09:14) | Self-only chat |
| 400 | 5 | handle 4 | `Hey` | seconds `812218500` (09:15) | Renamed direct |
| 500 | 2 | handle 1 | `Group hello` | seconds `812218560` (09:16) | Handle 2 sends nothing in the slice |
| 600 | 6 | handle 1 | `Tonight` | seconds `812218620` (09:17) | Named group |
| 700 | 1 | handle 1 | `After midnight` | seconds `812273400` (2026-09-28 00:30) | Outside slice A, inside slice B |

Message 116 owns guid `G112`. Row 112 is the reaction whose `associated_message_guid` is `p:0/G112`. The rowid and the guid are different numbers on purpose.

Attachments:

| attachment ROWID | message | transfer_name | filename | mime_type | is_sticker |
| --- | --- | --- | --- | --- | --- |
| 1 | 103 | `pic.jpg` | `/Users/ada/Library/Messages/Attachments/aa/bb/pic-on-disk.jpg` | `image/jpeg` | 0 |
| 2 | 104 | `heart.png` | `/Users/ada/Library/Messages/Attachments/aa/bb/heart.png` | `image/png` | 1 |
| 3 | 105 | null | null | null | 1 |

Expected chat-level fields:

| Chat | `display_title` | `empty_title_fallback` | `source_url` |
| --- | --- | --- | --- |
| 1 | `Ada Lovelace` | `+15551212` | `imessage://%2B15551212` |
| 2 | `Ada Lovelace, ada@icloud.com` | `Ada Lovelace, ada@icloud.com` | `imessage://%2B15551212,ada%40icloud.com` |
| 3 | `+15559876` | `+15559876` | `sms://%2B15559876` |
| 4 | `Me` | `Me` | `messages://` |
| 5 | `Sam` | `+15550000` | `imessage://%2B15550000` |
| 6 | `Book club` | `Ada Lovelace, ada@icloud.com` | `imessage://%2B15551212,ada%40icloud.com` |

Chat 2 front matter is the YAML block in the previous section. Chat 2's URL contains `ada@icloud.com` even though that handle sent no row in slice A. Chat 1's participants are Ada (`name: "Ada Lovelace"`) and `me@icloud.com` with `self: true`, and they do not contain `other@icloud.com`. Chat 4's only participant is `me@icloud.com` with `self: true` and no `name`. Chat 5's other participant has `name: "Sam"`. Chat 6's self handle still resolves through `last_addressed_handle`.

Section expectations that the fixture locks in:

- Row 100 heading `## 2026-09-27 09:01:00 -0700 — [[Ada Lovelace]]`, `- Id: 100`, body `See you there.`. The garbage `attributedBody` is not decoded.
- Row 101 heading ends with `— Me` and the same local time `09:02:00` from the nanosecond value.
- Row 102 body line `Reaction: Like to "dinner at 7"`.
- Row 103 body ends at `- Id: 103` and its attachment filename is `pic.jpg` with media type `image/jpeg`.
- Row 104 body line `Sticker: heart.png`, and the attachment list also carries `heart.png` / `image/png`.
- Row 105 body line `Sticker: sticker`, attachment filename absent, media type absent.
- Row 106 body line `Reaction: Love to "pic.jpg"`.
- Row 107 body line `Reaction: Laugh to "a message"`.
- Row 112 body line `Reaction: Emphasize to "<79 a's><U+00E9>"`. The trailing `Z` is cut. No ellipsis.
- Row 113 body line `Reaction: Cheer to "See you there."`.
- Rows 108, 109, 110, 115, and 116 produce no item in slice A. The rendered items do not contain `secret unsent body`, `secret retracted body`, or `too early`.
- Row 111 `file_day` is 2026-09-27. Row 700 is absent from slice A. In slice B, row 700 is present, `file_day` is 2026-09-28, and its heading time is `2026-09-28 00:30:00 -0700`.
- `tie_break` for row 100 is `00000000000000000100`. `stable_id` for chat 1 is `iMessage;-;+15551212`.

## Tests

Put them in `src/sources/messages.rs` under `#[cfg(test)]`, loading the SQL via `CARGO_MANIFEST_DIR`. The crate stays a binary; do not add `src/lib.rs` for these tests. `cargo test` of this module passes on Linux.

These tests must pass:

| Test | What it asserts |
| --- | --- |
| `apple_epoch_cutoff` | `812217660` and `812217660000000000` are the same UTC instant. `10^15` is seconds. `10^15 + 1` is nanoseconds. |
| `uri_is_readonly` | `/Users/ada/Library/Messages/chat.db` becomes `file:/Users/ada/Library/Messages/chat.db?mode=ro`. The URI does not contain `immutable`. A space in the path is `%20`. |
| `open_error_reasons` | Busy/locked, `Operation not permitted`, and a missing-file `errmsg` map to the three open reasons. None of the reasons contain `See you there.` |
| `slice_uses_sent_time` | Slice A includes row 100 and excludes row 110. `date_read` does not pull 110 in or push 100 out. |
| `file_day_follows_sent_instant` | Row 111 is 2026-09-27. Row 700 under slice B is 2026-09-28. |
| `nanosecond_heading_matches_seconds` | Row 101's heading time is `2026-09-27 09:02:00 -0700`. |
| `direct_group_sms_and_self_titles` | The six chats match the title, fallback, and `source_url` table, including whitespace-only group names and `Book club`. |
| `group_url_includes_silent_participant` | Chat 2's URL contains `ada%40icloud.com`. Handle 2 has no message in slice A. |
| `participants_yaml` | Chat 2 matches the YAML block, including sort order, omitted `name`, and `self: true`. Chat 1 excludes `other@icloud.com`. Chat 4 has only self. Chat 6 still has self via `last_addressed_handle`. |
| `one_item_per_message` | Slice A emits rows 100–107, 111–113, 200, 300, 400, 500, 600 and no others. Each of 100, 102, 104, and 103 is its own item. Items in chat 1 share `stable_id`, `display_title`, and `source_url`. |
| `tie_break_is_padded_rowid` | Row 100's `tie_break` is `00000000000000000100`. `"00000000000000000100" < "00000000000000000101"` as raw bytes. |
| `reaction_quote_and_names` | Like collapses whitespace. Love quotes `pic.jpg`. Laugh quotes `a message`. Emphasize keeps 80 scalars through U+00E9 and drops `Z`. Cheer uses the emoji column. A pure test of the name table covers Dislike and Question. |
| `sticker_and_attachment_metadata` | `Sticker: heart.png`, `Sticker: sticker`, `pic.jpg` wins over the on-disk basename, mime types match, and the sticker row is also in `attachments`. |
| `text_column_wins` | Row 100's body is `See you there.` while `attributedBody` is set. |
| `omit_unsent_and_retracted` | Rows 108 and 109 are absent, and no item body or error string built from this fetch contains `secret unsent body` or `secret retracted body`. |
| `messages_source_requires_macos` | `cfg(not(target_os = "macos"))`: `MessagesSource::fetch` returns `Messages is only available on macOS`. |

Also unit-test the pure encoder: `SMS;+;chat1` with handles `+15551212` and `ada@icloud.com` encodes as `sms://%2B15551212,ada%40icloud.com`, and a self-only list encodes as `messages://`.

## Assumptions

These are Messages choices. The tests above lock them in. The blank-line join of a chat file is foundation's group rule, not a second writer.

- `tie_break` is zero-padded to 20 digits so foundation's raw UTF-8 sort matches numeric `ROWID` order.
- Unsent means both "never sent" (`is_from_me` and `is_sent = 0`) and "retracted" (`date_retracted != 0`). Incoming rows are kept regardless of `is_sent`. Tapback removals (`3000..4000`) and non-zero `item_type` are omitted.
- Self is exact string match against `destination_caller_id`, then `account` with an `E:` or `P:` prefix stripped, then `last_addressed_handle`, then `account_login`. The first source that yields a value wins for that chat.
- Joined group titles use other participants only. `handle` has no display-name column. `chat.display_name` is the group name, and on a direct chat it is also the Messages display name used as the title's middle fallback and as the other participant's `name`.
- The quote truncates to 80 scalars with no ellipsis. `NSUnarchiver` failure clears that one body and does not fail the category.
