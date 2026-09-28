# 002 — Gmail

Implement `crate::sources::gmail` in `src/sources/gmail.rs` against [001-foundation.md](001-foundation.md). Source behavior is [../specs/gmail.md](../specs/gmail.md). This plan does not redefine the CLI, the lock, cursors, civil-date slicing, filename sanitizing, wiki-link matching, progress lines, directories, or the collect write phase.

Shared rules (exit table, slice bounds, layout, front-matter key order, the Open in Gmail line, `### Attachments`, contact matching) stay in the foundation plan. Gmail fills `CollectedItem` and owns `auth gmail` after foundation's client-file check.

## Module

Public surface:

- `GmailSource` implements `CollectSource`. `fetch` returns one `CollectedItem` per message whose `internalDate` falls in the slice. It does not merge a thread into one item and it does not group by directory.
- `authorize(secrets_dir: &Path) -> Result<(), GmailAuthError>` is what foundation calls after it has checked that `gmail-client.json` exists. Auth does not take the lock and does not read or write cursors.
- `GmailSource::group_extra_front_matter` overrides the trait default. It builds the single extra-YAML block for a file. Foundation passes the group it already sorted. The method does not sort items, does not look at `stable_id`, and does not open directories.

Private helpers stay in this module. No `cfg(target_os = "macos")` around HTTP, parsing, or HTML conversion. Tests run on Linux. No test calls `open`, EventKit, Contacts, or `chat.db`.

`GmailSource` holds the secrets directory and the run's `chrono_tz::Tz`. Production passes the same IANA zone the foundation clock uses. Tests pass `America/Los_Angeles`.

Construct an HTTP seam (`trait GmailHttp`) so `fetch` and `authorize` can run against fixtures. Production uses blocking `reqwest`. Library code returns errors. It does not call `process::exit`.

## Dependencies

Add these. Do not add `serde_yaml`, `tokio`, `yup-oauth2`, or `google-gmail1`.

```toml
reqwest = { version = "0.12", default-features = false, features = ["blocking", "json", "rustls-tls"] }
scraper = "0.22"
mailparse = "0.16"
base64 = "0.22"
sha2 = "0.10"
rand = "0.8"
encoding_rs = "0.8"
urlencoding = "2"
```

`reqwest` is blocking and rustls so `fetch` stays synchronous and Linux tests do not need OpenSSL. `yup-oauth2` writes access tokens into its store; `google-gmail1` pulls that stack in for two REST calls. The token POST is small enough to do directly, and `gmail.json` then contains only the refresh token. `scraper` is the HTML parser. The markdown converter is local; the reason is in [HTML to Markdown](#html-to-markdown).

`serde`, `serde_json`, `chrono`, and `chrono-tz` are already foundation dependencies.

## Secrets

| File | Who checks it | Contents |
| --- | --- | --- |
| `<vault>/.pkmagent/secrets/gmail-client.json` | Foundation exits `1` when the path is missing, before `authorize` or `fetch`. | Top-level `client_id` and `client_secret`. Extra keys are ignored. |
| `<vault>/.pkmagent/secrets/gmail.json` | Gmail. | One JSON object, one key: `refresh_token` (string). |

The spec's client file uses those two top-level keys. A Google console download that wraps them in `installed` or `web` does not match. Treat that as a missing key.

`gmail.json` stores the refresh token and nothing else. The account address is not stored. `fetch` reads it from `users.getProfile` (`emailAddress`) on every call.

Write `gmail.json` by creating a sibling temp file in the secrets directory, `set_permissions` to mode `0600` before the bytes are written, then rename over `gmail.json`. A failed rename removes the temp file. The file is compact JSON plus a trailing newline. Replacing an account means replacing the file. Do not merge with the previous token.

Pretty shape:

```json
{"refresh_token":"1//..."}
```

Never print a token, a client secret, an authorization code, or a message body. Logs and error strings are built from fixed sentences and, where noted, an HTTP status code.

## Auth

`authorize` reads `gmail-client.json`. Unreadable file, non-object JSON, or a missing or empty `client_id` or `client_secret`: return `GmailAuthError` with reason `gmail-client.json is invalid`. `main` maps every `GmailAuthError` to exit `1` and prints that reason on stderr by itself. Auth is not a collect job, so the line is not `YYYY-MM-DD failed gmail:`.

Installed-app loopback, scope exactly `https://www.googleapis.com/auth/gmail.readonly`.

1. Bind `127.0.0.1:0`. Redirect URI is `http://127.0.0.1:<port>/`. Use `127.0.0.1`, not `localhost`.
2. PKCE S256. Verifier is 32 random bytes, base64url without padding. Challenge is base64url-without-padding of the SHA-256 of the verifier ASCII. `state` is 16 random bytes, hex encoded.
3. Browser URL, query-encoded:

```text
https://accounts.google.com/o/oauth2/v2/auth
  ?client_id=<id>
  &redirect_uri=<redirect>
  &response_type=code
  &scope=https://www.googleapis.com/auth/gmail.readonly
  &access_type=offline
  &prompt=consent
  &code_challenge=<challenge>
  &code_challenge_method=S256
  &state=<state>
```

`prompt=consent` and `access_type=offline` are what make Google return a refresh token for an already-approved client.

4. Open that URL with `open` on macOS. Tests inject the opener and never spawn `open`. If the opener fails, print the URL on stderr and keep waiting. That URL contains the client id and does not contain the client secret.
5. Accept one callback. `GET /favicon.ico` is `404`; keep waiting. Read `code`, `error`, and `state`. Respond `200` `text/html; charset=utf-8` with `pkmagent authorized this Gmail account. You can close this window.` The HTML contains no token.
6. Timeout is five minutes. Reason: `Gmail authorization timed out`.
7. `error=access_denied` → `Gmail authorization was denied`. Any other `error` → `Gmail authorization failed`. `state` mismatch → `Gmail authorization state did not match`.
8. Exchange the code. `POST https://oauth2.googleapis.com/token` with `Content-Type: application/x-www-form-urlencoded` and `grant_type=authorization_code`, `code`, `client_id`, `client_secret`, `redirect_uri`, `code_verifier`.
9. On a non-success HTTP status, or JSON without a non-empty `refresh_token`: `Gmail did not return a refresh token` when the body parsed and the token was absent, otherwise `Gmail authorization failed`. Do not copy the response body into the reason.
10. Write `gmail.json` as above. Discard `access_token`. A write failure is `writing gmail.json failed`.

Bind failure and a token-endpoint transport failure use `Gmail authorization failed`.

## Fetch

`fetch(slice, contacts)` uses the stored refresh token and the same client id and secret.

1. Missing `gmail.json` → `Gmail is not authorized`.
2. Client file or token file unreadable, wrong shape, or empty `refresh_token` → `gmail-client.json is invalid` or `gmail.json is invalid`.
3. Refresh: `POST https://oauth2.googleapis.com/token` with `grant_type=refresh_token`, `refresh_token`, `client_id`, `client_secret`. Hold `access_token` in memory. If the response also contains a new non-empty `refresh_token`, rewrite `gmail.json` the same way, still with only that key. Refresh failure → `refreshing the Gmail credential failed`.
4. `GET https://gmail.googleapis.com/gmail/v1/users/me/profile` with `Authorization: Bearer`. Read `emailAddress`. Failure or an empty address → `reading the Gmail profile failed`. That string is the account for `source_url` and for `account:`.
5. List messages, then `GET` each id with `format=full`. Map, then keep items with `slice.start <= t < slice.end`. Return them in any order.

A failure at any step fails the whole fetch. Return no partial `Vec`. Foundation turns `SourceError` into exit `2` and prints `YYYY-MM-DD failed gmail: <reason>`. The reason is the `SourceError` display text alone: no date, no `gmail:` prefix, no response body, no token, no subject, no snippet.

| Failure | Reason |
| --- | --- |
| `gmail.json` absent | `Gmail is not authorized` |
| bad client file during fetch | `gmail-client.json is invalid` |
| bad token file | `gmail.json is invalid` |
| refresh HTTP or transport error, or no `access_token` | `refreshing the Gmail credential failed` |
| profile error | `reading the Gmail profile failed` |
| list HTTP or transport error | `listing Gmail messages failed (HTTP <status>)` when a status exists, otherwise `listing Gmail messages failed` |
| repeated `nextPageToken` | `Gmail list pagination repeated a page token` |
| message GET error | `reading a Gmail message failed (HTTP <status>)` or `reading a Gmail message failed` |
| missing `internalDate` | `a Gmail message has no internalDate` |
| missing `threadId` | `a Gmail message has no thread id` |

Status codes are allowed in those reasons. Response bodies are not.

### Query

List:

```text
GET https://gmail.googleapis.com/gmail/v1/users/me/messages
  maxResults=500
  includeSpamTrash=false
  q=<query>
  pageToken=<only when continuing>
```

`500` is the API maximum. Follow `nextPageToken` until it is absent. Remember tokens for the call; a repeat is the pagination error above. An absent `messages` array is an empty page, and an empty list is a successful empty `Vec`.

Query string, one space between terms:

```text
-in:drafts -in:spam -in:trash after:<start_sec - 1> before:<end_sec + 1>
```

`start_sec` and `end_sec` are `slice.start.timestamp()` and `slice.end.timestamp()`. Gmail's `after` and `before` are second-granularity. Widening by one second makes the list a superset whichever way those operators round. The millisecond compare against the slice is the filter that implements `slice_start <= t < slice_end`. Foundation drops out-of-slice items again.

Do not add `in:inbox` or `label:inbox`. Archived mail and sent mail are in the default corpus. `includeSpamTrash=false` plus `-in:spam` and `-in:trash` exclude Spam and Trash. `-in:drafts` excludes Drafts; the spam/trash flag does not. Do not post-filter on `labelIds`. Exclusion is this query. The mapper still maps a message whose labels are only `SENT` or only a category label.

Do not call `threads.get`, `history.list`, or `messages.attachments.get` for files. Do not send a list request without this `q`. There is no full-mailbox path.

Worked query, zone `America/Los_Angeles`, slice local `2026-09-26` inclusive through local `2026-09-27` exclusive (`1790406000` .. `1790492400`):

```text
-in:drafts -in:spam -in:trash after:1790405999 before:1790492401
```

### Message get

```text
GET https://gmail.googleapis.com/gmail/v1/users/me/messages/<id>?format=full
```

`format=full` returns headers, body part data, filenames, and mime types. It does not return file bytes. `format=metadata` has no body. `format=raw` is unused.

Use the message resource's `id`, `threadId`, `internalDate`, and `payload`. Ignore `snippet`, `labelIds`, `historyId`, and `sizeEstimate` when deciding what to import.

### internalDate

`internalDate` is epoch milliseconds, as a decimal string. `t` is that instant, not the `Date` header.

```rust
let utc = DateTime::<Utc>::from_timestamp_millis(millis)?;
let zoned = utc.with_timezone(&tz);
let t: DateTime<FixedOffset> = zoned.fixed_offset();
let file_day = zoned.date_naive();
```

The offset is the zone's offset at that instant, so a fall-back hour keeps the offset that was in effect then. `file_day` is the civil date of `t` in that zone. Compare `t` to the slice as instants.

A message whose `Date` header falls on another civil day is included when `internalDate` is inside the slice. The heading clock is `t`, formatted `%Y-%m-%d %H:%M:%S %z` (`2026-09-26 08:14:03 -0700`).

## CollectedItem

| Field | Value |
| --- | --- |
| `stable_id` | `threadId` |
| `display_title` | that message's subject, or `""` when the subject is empty |
| `empty_title_fallback` | `(no subject)` |
| `source_url` | `https://mail.google.com/mail/?authuser=<account>#all/<thread id>` |
| `t` | `internalDate` as `DateTime<FixedOffset>` in the run zone |
| `tie_break` | Gmail API message `id` |
| `file_day` | local civil date of `t` |
| `extra_front_matter` | this message's `account` and `participants` YAML |
| `body` | this message's section, with no source link and no attachments heading |
| `attachments` | this message's filename and media-type metadata |

Empty subject: the `Subject` header is missing, or the value is empty after replacing CR, LF, and tab with a space and trimming the ends. `display_title` stays `""`. The section's Subject line uses `(no subject)`. Keep `Re:` and `Fwd:` in a non-empty subject. Do not prefer an earlier message's subject.

Percent-encode the account with `urlencoding` (`@` → `%40`, `+` → `%2B`). Insert the thread id as raw bytes. `name@gmail.com` and thread `18c2f0a1b2c3d4e5` produce:

```text
https://mail.google.com/mail/?authuser=name%40gmail.com#all/18c2f0a1b2c3d4e5
```

A thread id `a+b` stays `a+b`. Every message in the thread stores that same URL. The account scalar in YAML is the raw address, not the encoded form.

`tie_break` is the API `id`. Foundation sorts equal `t` by `tie_break` ascending raw UTF-8. That order is the message order inside the file: oldest `internalDate` first, message id ascending on a tie. The chronologically last message is the last item after that sort.

### File title

Every item carries its own subject in `display_title`. `empty_title_fallback` is `(no subject)`. Foundation names the file from the last item after sorting by `t` then `tie_break`, then applies that item's fallback, so an empty last subject becomes `(no subject)`. Gmail does not pick the filename and does not run a second grouping pass.

A flat run (one or two civil dates) puts both days of a thread in one file; the title is the later message's subject, including `Re:` or `Fwd:`. A dated run (more than two civil dates) puts each civil date's messages in their own file; each file's title is the last message among the items foundation placed in that directory. Gmail does not branch on flat versus dated.

## Extra front matter

Per message, rendered YAML, no document markers, trailing newline, inserted by foundation after the shared keys:

```yaml
account: name@gmail.com
participants:
  - name: "Ada Lovelace"
    email: ada@example.com
```

`participants` on an item are From, To, Cc, and Bcc of that message only. Walk those headers in that order. Repeated copies of one of those headers are concatenated with `, ` in appearance order. Header names match ASCII case-insensitively. Parse with `mailparse::addrparse` so quoted commas stay inside one mailbox, and so RFC 2047 display names decode. Flatten groups to their mailboxes. Skip a mailbox whose trimmed address is empty.

Within the message, unique by trimmed email, case-insensitive. Two display names for one email: keep the first in the header walk above. Then sort the kept entries by the ASCII-lowercased email, raw UTF-8. Preserve the first-seen spelling of the email.

`name` is `ContactIndex::title_for_email` when that returns a title. Otherwise it is the trimmed header display name. Omit the `name` line when both are absent. The contact title is a plain string. Apply `title_for_email` while building the item. The index is the one `fetch` received.

Always write `account`. Omit `participants` when the message has none.

Quote `name` with double quotes. Escape `\`, `"`, and replace newlines in the name with a space. Write `account` and `email` as plain scalars when every byte is ASCII alphanumeric or `.!#$%&'*+/=?^_`{|}~@+-`. Otherwise double-quote them the same way.

### File-level participants

The spec's `participants` list is every From, To, Cc, and Bcc on the messages in the file, unique by trimmed email, case-insensitive, sorted by email. The first display name in message order wins. A flat two-date collect builds that file from two `fetch` calls, so `fetch` cannot see both days. Foundation calls `group_extra_front_matter` on the sorted group instead of pasting one item's YAML.

1. Input is the sorted group for one file.
2. `account` is the first item's account. One Gmail account per run.
3. Parse each item's rigid participant list in order. Unique by trimmed email, case-insensitive, keeping the first resolved `name` and the first email spelling.
4. Sort that union by ASCII-lowercased email, raw UTF-8.
5. Emit one YAML block in the form above. Omit `participants` when the union is empty.

A contact match is already resolved on each item, and the same email resolves to the same title on every item, so the kept name is the contact title when the index matches. Header-name collisions only happen for addresses the index does not match.

Foundation inserts this string once. It does not concatenate per-item YAML. A one-item group round-trips that item's block.

## Message sections

`body` is one section. Foundation writes `[Open in Gmail](source_url)` before it and `### Attachments` after it.

```markdown
## 2026-09-26 08:14:03 -0700 — [[Ada Lovelace]] <ada@example.com>

- Message-Id: 18c2f0a1b2c3d4e5
- Subject: Quarterly plan
- To: you@example.com
- Cc: [[Ada Lovelace]] <ada@example.com>

Body of the message.
```

The timestamp uses the em dash `—` (U+2014) with spaces around it. `Message-Id` is the API message `id`. Ignore the RFC 5322 `Message-ID` header. The Subject line is always present. Empty subject prints `(no subject)`.

Address lines, in order: To, then Cc, then Bcc. Always emit To. Omit Cc when it has no mailboxes. Omit Bcc when it has no mailboxes. Join mailboxes with `, `.

Body form of one mailbox, using `title_for_email` while building the section:

- A match is `[[<title>]] <trimmed-email>`.
- The unmatched account address, and only as the heading sender, is `Me`. Compare the trimmed From address to the profile address, ASCII case-insensitive.
- Any other unmatched mailbox with a display name is `<display name> <trimmed-email>`.
- Any other unmatched mailbox without a display name is the bare trimmed email.

The heading sender is the first From mailbox. No From mailbox: the heading is `## <timestamp>` with no dash. To, Cc, and Bcc do not use `Me`. An unmatched account in To stays in the mailbox form above. Front matter does not use `Me`.

The section body follows a blank line after the list. It is the markdown from [HTML to Markdown](#html-to-markdown). When that result is empty, the section ends after the list. The item `body` has no leading newline and one trailing newline.

A thread file has one section per message. Foundation joins item bodies in the sorted group with a blank line between them, then writes one attachments list. Gmail does not concatenate inside `fetch`.

## HTML to Markdown

Prefer the HTML body. Walk the `payload` tree depth-first. Skip `multipart/*` containers and recurse into their parts.

A part is a file part when any of these hold:

- the filename is non-empty
- `Content-Disposition`'s type is `attachment`
- `body.attachmentId` is set and the mime type is neither `text/html` nor `text/plain`

The HTML body is the first `text/html` part that is not a file part. The plain body is the first `text/plain` part that is not a file part. A large text body may carry `attachmentId` and still be the body, because its mime type is `text/html` or `text/plain` and it has no filename and no attachment disposition.

Decode `body.data` as base64url. Try the unpadded engine, then the padded one. Decode the bytes with `encoding_rs` from the part's `Content-Type` charset, default UTF-8, lossy. Newlines become LF.

When the selected HTML or plain part has an `attachmentId` and no `data`, GET that one attachment and use the bytes as the body. That GET is the message text. Do not GET a file or image part.

Converter: a private walker over `scraper::Html`, not `htmd` and not `html2md`. Those crates keep ordinary tags and still need a Gmail-specific pass for `div.gmail_quote` and for lifting `cid:` images out of the body. The kept constructs are the spec's list, so the walker is smaller than wrapping a general converter and then correcting it. `scraper` is only the HTML5 parser.

`html_to_markdown` returns markdown plus the `cid` values in document order.

- Drop `script`, `style`, `noscript`, `head`, and `title`, including their text.
- Keep text inside ordinary `div` and `span`. Gmail HTML is mostly `div`s; dropping `div` text would throw away the HTML body and fall through to `text/plain`.
- `p`, a non-quote `div`, headings, lists, `blockquote`, and `pre` are blocks, joined by a blank line.
- `br` is a hard break: two spaces and a newline inside the block.
- `h1` through `h6` become that many `#` characters.
- `ul` / `li` becomes `- `. `ol` / `li` becomes `1. `, `2. `, and so on. Indent a nested list by two spaces.
- `strong` and `b` become `**`. `em` and `i` become `*`.
- `a` with an `href` becomes `[text](href)`. Empty text uses the href as the text. No href: keep the text.
- `blockquote`, and a `div` whose class tokens include exactly `gmail_quote`, are quotes. Prefix each rendered line with `> `. A nested quote is prefixed again. Class tokens split on ASCII whitespace, so `gmail_quote_container` is not `gmail_quote`. An empty quote emits nothing.
- A remote image (`img` whose `src` does not start with `cid:`, ASCII case-insensitive) becomes `![alt](src)`. Alt newlines become spaces. This includes `http`, `https`, and `data:` sources. Do not fetch them.
- A `cid:` image is removed from the markdown. Record the id after `cid:`, trimmed, surrounding `<>` removed.
- Other tags contribute their children. Table cells in one row are separated by a space, rows by a newline. Do not build a markdown table.
- Turn NBSP into a normal space. Collapse runs of spaces and tabs inside a text node to one space. `trim()` on the finished markdown decides emptiness.

Quoted history stays. Do not strip `gmail_quote`, and do not strip plain-text lines that start with `>`.

If there is no HTML part, or the conversion's `trim()` is empty, use `text/plain` when its `trim()` is non-empty. Normalize newlines, keep leading whitespace, and trim trailing whitespace. Otherwise the section body is empty.

Fixture `tests/fixtures/gmail/quote.html` converts to `tests/fixtures/gmail/quote.md`:

```html
<div dir="ltr"><p>Hello</p>
<p>See you<br>there</p>
<h2>Plan</h2>
<ul><li>One</li><li>Two</li></ul>
<p><strong>bold</strong> and <em>italic</em> <a href="https://example.com/x">here</a></p>
<blockquote><p>A quote</p></blockquote>
<div class="gmail_quote"><div class="gmail_attr">On Mon, Ada wrote:</div><blockquote class="gmail_quote">Previous note</blockquote></div>
<script>secret()</script><style>p{color:red}</style>
<img alt="pic" src="https://example.com/a.png"><img alt="inline" src="cid:ii_abc"></div>
```

```markdown
Hello

See you  
there

## Plan

- One
- Two

**bold** and *italic* [here](https://example.com/x)

> A quote

> On Mon, Ada wrote:
>
> > Previous note

![pic](https://example.com/a.png)
```

`secret` and `color:red` are absent. `cid:ii_abc` is absent from the markdown and present in the cid list. The hard break after `See you` is two trailing spaces.

## Attachments

Metadata only. Foundation's `Attachment` is `filename: Option<String>` and `media_type: Option<String>`. `None` filename is what foundation prints as `unnamed`. `None` media type means omit it. Empty or whitespace filename is `None`. Media type is the mime type with parameters removed (`image/png` from `image/png; name="photo.png"`). Missing mime type is `None`.

Collect file parts in payload depth-first order. The selected HTML part and the selected plain part are not attachments, including when the body was loaded through `attachmentId`. Match a `cid` to a part by `Content-ID`, case-insensitive, after trimming and stripping one layer of `<>`. A matched part is one attachment, using that part's filename and media type. A `cid` with no part is an extra attachment with both fields `None`, after the file parts, in document order. Do not emit a second row for the matched cid.

Each item carries only its own attachments. Foundation appends one `### Attachments` list for the file, walking items in sorted order and each item's attachments in the order collected.

## What Gmail stores on each item

Foundation groups by `stable_id`, sorts by `t` then `tie_break`, and applies the group rules in [001-foundation.md](001-foundation.md). Gmail's side of that contract:

- Every item in a thread stores the same `source_url`, so the first item's URL is the file's URL.
- `GmailSource::group_extra_front_matter` is the file's extra YAML.
- Each item `body` is one section. Foundation joins them.
- Each item carries only its own attachments. Foundation appends one list.

`day` stays the civil date of the earliest item. Gmail only sets per-item `file_day`.

## Tests

`cargo test` on Linux, fixtures under `tests/fixtures/gmail/`, helpers loaded via `CARGO_MANIFEST_DIR`. No test prompts TCC. These pass before Gmail is done.

| Test | What it locks |
| --- | --- |
| `list_query_excludes_spam_trash_and_drafts` | The worked query above, `includeSpamTrash=false`, `maxResults=500`, and the absence of `in:inbox`. |
| `maps_thread_across_two_civil_dates` | `thread-two-days.json`: thread `t1`, message `m1` at `internalDate` `1790435643000` (`2026-09-26 08:14:03 -0700`, subject `Quarterly plan`), message `m2` at `1790524800000` (`2026-09-27 09:00:00 -0700`, subject `Re: Quarterly plan`). Same `stable_id`, `tie_break` `m1` and `m2`, those `display_title`s, those `file_day`s. After the foundation sort, the last `display_title` is `Re: Quarterly plan`. The test does not create directories. A flat run files both messages together; a dated run files them on their `file_day`s. |
| `internal_date_not_date_header` | `date-header-ignored.json`: `Date` is `Tue, 01 Jan 2020 00:00:00 +0000` and `internalDate` is `1790435643000`. `t`, `file_day`, and the heading use `internalDate`. |
| `slice_bounds_use_internal_date` | Exactly `slice.start` is kept, one millisecond before it is dropped, one millisecond before `slice.end` is kept, exactly `slice.end` is dropped. |
| `gmail_quote_html_to_markdown` | `quote.html` equals `quote.md`, and the cid list is `ii_abc`. |
| `cid_image_is_attachment_metadata_only` | `cid-message.json`: inline `image/png` filename `photo.png`, `Content-ID` `<ii_abc>`, `attachmentId` set; an `application/pdf` part with `attachmentId` and no filename; HTML also references `cid:missing`. Attachments are `photo.png` / `image/png`, then `None` / `application/pdf`, then `None` / `None`. The body has no `cid:`. The HTTP fake records no `/attachments/` GET for those file parts. |
| `empty_subject` | `empty-subject.json` has no `Subject`. `display_title` is `""`, `empty_title_fallback` is `(no subject)`, the section contains `- Subject: (no subject)`. |
| `plain_text_when_html_missing_or_whitespace` | No HTML part: the section body is the plain text, including a line that starts with `>`. HTML that converts to whitespace, with plain text present: the plain text. Both whitespace: the section body is empty and the Subject line remains. |
| `participants_and_me` | Load a temp `contacts/` directory through `ContactIndex`. One person is titled `Ada Lovelace` and has email `ada@example.com`. Message `m1`: From `Ada B <Ada@example.com>`, To `You <you@example.com>` (the account) and `Robert <bob@example.com>`, Cc `Ada A <ada@example.com>`. Bcc is empty, so the section has no Bcc line. Message `m2` is later: From `Other Ada <ada@example.com>`, To `Bob <bob@example.com>`. `m1`'s YAML name for `ada@example.com` is the plain contact title `Ada Lovelace`, and its heading sender is `[[Ada Lovelace]] <Ada@example.com>`. A message whose From is the unmatched account uses the heading `Me`. `bob@example.com` matches no contact. `group_extra_front_matter` on the oldest-first pair keeps `Robert` (the first display name), includes addresses from both messages, and sorts by email. |
| `sent_message_maps` | A payload whose `labelIds` are `["SENT"]` and whose From is the account still becomes an item. |
| `source_url_encodes_account_not_thread_id` | The `name@gmail.com` URL above, and thread id `a+b` unencoded. |
| `authorize_stores_refresh_token_only` | Injected opener (no `open`) hits the loopback on `127.0.0.1`. Token JSON contains `access_token`, `refresh_token`, and `super-secret-token-value`. `gmail.json` contains the refresh token, does not contain the access token or that secret, mode is `0600`, and a previous refresh token is gone. |
| `error_reason_omits_response_body` | A 500 body of `super-secret-token-value` plus a sentence of message text. The `SourceError` display does not contain that secret or that sentence. |

`thread-two-days.json` is two `format=full` message resources. Each `Date` header is `Tue, 01 Jan 2020 00:00:00 +0000` so the two-day test also fails a mapper that trusts `Date`. Subjects stay `Quarterly plan` and `Re: Quarterly plan`.

## Out of scope

- More than one Gmail account. `gmail.json` is one refresh token. Authorizing again replaces it.
- A full-mailbox import. `collect` passes a slice, and the list query always carries `after` and `before`. `import gmail` stays rejected by foundation.
- The Google Calendar API, and any OAuth scope other than `https://www.googleapis.com/auth/gmail.readonly`.
- Downloading attachment bytes, printing tokens or message bodies, and `serde_yaml`.

## Assumptions

These are Gmail choices. Grouping, the attachment record, and the temp-and-rename of `gmail.json` are the foundation contract.

- The account address comes from `users.getProfile`, not from `gmail.json`.
- Auth failures exit `1` with a bare reason. The dated `failed gmail:` line is only for a collect job.
- The client file is top-level `client_id` and `client_secret`. A download wrapped in `installed` or `web` is invalid.
- `Me` is the heading sender when the account address does not match a contact. Recipients keep the mailbox form. Front matter does not use `Me`.
- A rotated `refresh_token` on a refresh response replaces `gmail.json`.
