# 005 — Contacts

Implementation of [../specs/contacts.md](../specs/contacts.md) against [001-foundation.md](001-foundation.md). Contacts is the import snapshot. `src/sources/contacts.rs` owns the fetch, the record, the extra front matter, and the create-versus-refresh writer. `src/collect.rs` calls that writer from the existing import path, under the one lock foundation already takes.

`collect contacts` stays rejected in `src/cli.rs`. This module has no cursor, no `--since`, and no `--day`. It does not read or write `state.json` and it does not create an inbox directory.

## Module

```text
src/sources/contacts.rs
tests/fixtures/contacts/
```

```rust
pub struct ContactRecord {
    pub id: String,
    pub display_title: String,
    pub source_url: String,
    pub extra_front_matter: String,
    pub note_body: String,
}

pub fn import_contacts(
    contacts_dir: &Path,
    snapshot: Result<Vec<ContactRecord>, SourceError>,
    imported_at: &str,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<u64, SourceError>

pub fn fetch_snapshot() -> Result<Vec<ContactRecord>, SourceError>
```

`fetch_snapshot` exists on every target. The `CNContactStore` body is `cfg(target_os = "macos")`. Any other target returns `Contacts is only available on macOS` and does not create a directory. `SourceError` is the existing source error. Its reason is the text foundation prints after `contacts failed: `. Library code returns that error. It does not call `process::exit`.

`note_body` is the markdown foundation writes after the source link. `extra_front_matter` is YAML with no document markers. Foundation inserts it after the shared keys and does not reorder it.

The Linux build compiles the fixture mapper, `import_contacts`, and the non-macOS `fetch_snapshot` error. Tests call `import_contacts` with records they already built. No test calls `CNContactStore`.

## Fetch

`fetch_snapshot` opens `CNContactStore` and enumerates people. It does not call group or container APIs, and the fetch predicate stays `nil`, so the store is not limited to one group or one container.

Use `CNContactFetchRequest`:

- `predicate`: `nil` (every contact)
- `unifyResults`: `true`
- `mutableObjects`: `false`
- `sortOrder`: left at `CNContactSortOrderNone`; this module sorts by id itself

`unifyResults` stays true because that is the person Contacts.app shows. Linked cards from separate accounts are one person and become one file. The id stored is `CNContact.identifier` on that unified contact. Apple's identifier can be saved and passed back to `predicateForContactsWithIdentifiers` to fetch the same unified person. Individual linked-contact ids are not written.

Enumerate with `enumerateContactsWithFetchRequest:error:usingBlock:`. The block never sets `stop`. A `false` return discards every contact collected in that call and becomes `Err`. An empty address book is `Ok(vec![])`.

Keys to fetch, as `CNKeyDescriptor` values:

- `CNContactFormatter::descriptorForRequiredKeysForStyle(CNContactFormatterStyle::FullName)`
- `CNContactIdentifierKey`
- `CNContactTypeKey`
- `CNContactOrganizationNameKey`
- `CNContactJobTitleKey`
- `CNContactBirthdayKey`
- `CNContactPhoneNumbersKey`
- `CNContactEmailAddressesKey`
- `CNContactPostalAddressesKey`
- `CNContactUrlAddressesKey`
- `CNContactNoteKey`

Do not fetch images, relations, social profiles, instant-message addresses, or dates other than birthday.

Before enumerate, read `authorizationStatusForEntityType(CNEntityTypeContacts)`:

| Status | Result |
| --- | --- |
| Denied | `Err` reason `access denied`. Do not enumerate. |
| Restricted | `Err` reason `access restricted`. Do not enumerate. |
| Authorized | Enumerate. |
| NotDetermined | Enumerate. macOS prompts only when the binary's `Info.plist` contains `NSContactsUsageDescription`, which [001-foundation.md](001-foundation.md) embeds in the same section as the calendar keys. A denial comes back as an error and uses the reason `access denied`. |

Any other store error uses the reason `fetch failed (<domain> <code>)`. Do not put `localizedDescription` or `userInfo` in the reason. Those can contain contact data. The reason has no trailing newline.

Map each returned `CNContact` into a `ContactRecord` with the pure functions below, then return the vec. A contact with an empty identifier is dropped. Stable-sort by id bytes and, when two records share an id, keep the first. Enumeration order is the tie-break.

## Display title

`display_title` is chosen before filename sanitizing. Collapse whitespace the same way filename step 1 does, and use that only to choose the string:

1. `CNContactFormatter::stringFromContact_style(contact, CNContactFormatterStyle::FullName)`, when the collapsed string is non-empty.
2. Otherwise, when `contactType` is `CNContactTypeOrganization` and the collapsed `organizationName` is non-empty, that organization name.
3. Otherwise the display title is empty. `empty_title_fallback` passed to the filename allocator is `untitled`.

A company record is `CNContactTypeOrganization`. The organization string stored in front matter is the raw `organizationName`, including surrounding spaces. The collapsed copy is only the filename and progress label.

The progress label is step 1 or step 2, or `untitled` when the display title is empty. It is not the collision suffix. This module passes that label to the progress callback. It does not format the status line.

## Source link

```rust
pub fn source_url(id: &str) -> String
```

`addressbook://` plus the identifier percent-encoded as UTF-8. Leave RFC 3986 unreserved bytes (`A-Z a-z 0-9 - . _ ~`) as they are. Encode every other byte as `%` and two uppercase hex digits. Encode the raw identifier once. The `id` front-matter field stays the raw identifier. Foundation writes the link text `Open in Contacts`.

## Fields on the record

Pure functions take owned strings, so the fixture loader and the macOS mapper share them.

### Lists

`phones`, `emails`, `addresses`, and `urls` are lists of `{ label, value }`.

- Phone `value` is `CNPhoneNumber::stringValue`. Do not read the digits-only property and do not reformat.
- Email `value` is the labeled value's string. Do not trim and do not change case.
- URL `value` is the labeled value's string.
- Address `value` is `format_address` below.
- `label` is `CNLabeledValue::localizedStringForLabel`. A nil label is `""`. Store `mobile`, `home`, `work`, and `homepage`, which is what that call returns for the standard labels in English. Do not store the `_$!<Mobile>!$_` constants.

Drop an item whose `value` is empty. Keep an item whose label is empty. Do not dedupe. Sort each list by `label` bytes, then `value` bytes, raw UTF-8 (`Ord` on `str`).

`ContactIndex` reads `phones[].value` and `emails[].value` from this YAML. Matching rules stay in foundation.

### Addresses

```rust
pub fn format_address(
    street: &str,
    sub_locality: &str,
    city: &str,
    sub_administrative_area: &str,
    state: &str,
    postal_code: &str,
    country: &str,
) -> String
```

Take those `CNPostalAddress` components in that order. In each component, replace `\r\n`, `\n`, and `\r` with `, `, then trim ASCII spaces. Drop an empty component. Join the rest with `, `. `123 Main St` and `London` become `123 Main St, London`. Do not call `CNPostalAddressFormatter`; its mailing style is locale-specific and multiline. `ISOCountryCode` is not a component.

### Birthday

```rust
pub fn format_birthday(year: Option<i32>, month: i32, day: i32) -> Option<String>
```

Month must be `1..=12` and day `1..=31`, or the field is omitted. Do not check that the day exists in that month. A missing year formats `--MM-DD` (`--03-07`). A year `>= 0` formats `YYYY-MM-DD` with the year padded to at least four digits (`0999-01-01`, `1815-12-10`). A negative year omits the field.

On macOS, `CNContact::birthday` is `NSDateComponents`. `year`, `month`, or `day` equal to `NSDateComponentUndefined` (`NSIntegerMax`) is missing. A missing month or day omits `birthday`. A missing year is `None`.

### Note

The note is `CNContact::note`. It is not a front-matter field. Strip trailing `\n` and `\r`. If nothing remains, or only Unicode whitespace remains, `note_body` is empty. Otherwise `note_body` is `\n` plus that text, with no trailing newline of its own. Internal newlines stay. Leading whitespace stays when the note also has a non-whitespace character. A note of only spaces is empty, which is what `omit_empty_note` locks in.

## Extra front matter

`render_extra` writes these keys, in this order, and omits a key when the string is empty or the list is empty:

```yaml
organization: "Analytical Engines"
job_title: "Mathematician"
birthday: "1815-12-10"
phones:
  - label: "mobile"
    value: "+15551212"
emails:
  - label: "home"
    value: "ada@example.com"
addresses:
  - label: "work"
    value: "123 Main St, London"
urls:
  - label: "homepage"
    value: "https://example.com"
```

Every scalar is a YAML double-quoted string. Escape `\` as `\\`, `"` as `\"`, newline as `\n`, CR as `\r`, and tab as `\t`. Any other byte below `0x20` is `\u00` plus two uppercase hex digits. Lists use two-space indentation, `label` then `value`. The string ends with one `\n` when it contains a key, and is empty when every key was omitted. No `---` markers. Do not use `serde_yaml`.

## New file

Foundation's note writer owns `source`, `id`, `title`, `source_url`, and `imported_at`, quotes `id`, `title`, and `source_url`, and omits `day` and `timezone` for `source: contacts`. Pass `note_body` as the body. The writer adds `[Open in Contacts](<source_url>)` and exactly one trailing newline on the file.

A record with a note produces:

```markdown
---
source: contacts
id: "00000000-0000-0000-0000-000000000001"
title: "Ada Lovelace"
source_url: "addressbook://00000000-0000-0000-0000-000000000001"
imported_at: 2026-09-27T18:04:11-07:00
organization: "Analytical Engines"
job_title: "Mathematician"
birthday: "1815-12-10"
phones:
  - label: "mobile"
    value: "+15551212"
emails:
  - label: "home"
    value: "ada@example.com"
addresses:
  - label: "work"
    value: "123 Main St, London"
urls:
  - label: "homepage"
    value: "https://example.com"
---
[Open in Contacts](addressbook://00000000-0000-0000-0000-000000000001)

Note text.
```

An empty `note_body` stops after the link line. There is no extra blank line. `imported_at` on a new file is the run's frozen time, the string `import_contacts` receives. `title` is the allocated filename without `.md`.

## Import

`src/collect.rs` runs import after argument parsing, the lock, and the frozen timestamp. The lock may create `<vault>/.pkmagent/`. Import then calls `fetch_snapshot`, then `import_contacts` with `<vault>/contacts` and that timestamp. It does not open `state.json` and does not create the inbox. `import_contacts` does not create `.pkmagent/`. The count collect prints is the `u64` returned here. Foundation prints `contacts=<files>`.

`import_contacts`:

1. On `Err`, return that error before `create_dir_all`. Leave `contacts/` as it is, including when the directory is absent.
2. On `Ok`, `create_dir_all` the directory. A failure here returns `write failed: <io>` and deletes nothing.
3. Sort the records by id, raw UTF-8.
4. Read the directory one level deep. Do not recurse. Ignore non-UTF-8 names.
5. Write every record. After each successful write, call `progress(done, total, label)` with `total` equal to the number of records and `label` the display title from above. `done` starts at 0 and increments by one per record.
6. After every write has succeeded, delete. A write error returns immediately, leaves the files already written, and skips deletion.

Writes use `std::fs::write` on the final path. That truncates in place. There is no sibling temp file and no `rename`.

`<files>` counts creates and updates. A record that already has a file is an update even when the new bytes match the old bytes. Deletions are not included.

### Which path is updated

Markdown here means the file name ends with the bytes `.md`. Collect every markdown file in `contacts/` whose first front matter has an `id`. Group those paths by that id.

The import step updates the path that sorts first by raw UTF-8 bytes of the relative file name. Other files with the same id are left byte-for-byte unchanged, and they are not deleted, because the id is in the snapshot.

A record with no chosen path is a create. Call the foundation filename allocator with `display_title`, `empty_title_fallback` of `untitled`, and the stable id. Names already in the directory are taken, including a markdown file this run will delete after the writes. Creates and updates run in ascending id order so collision suffixes stay deterministic. Import never renames.

### Front matter splice

Read the whole existing file. Find the first front matter and keep every byte after its closing delimiter.

The opening delimiter is the bytes `---\n` or `---\r\n` at the start of the file. Scan lines after that. A line is the bytes before the next `\n`, and the line may be the tail of the file when there is no `\n`. The closing delimiter is the first line whose content is `---` or `---\r`. The body is:

- every byte after the `\n` that ends that line, or
- `&[]` when the closing `---` is the last line and has no `\n`.

Those body bytes are kept as they are, including a missing trailing newline and any `\r` inside the body. A later `---` in the body is not a delimiter.

If the opening or closing delimiter is missing, return `write failed: front matter missing`, do not change that file, and do not run the delete pass.

Render a replacement through the foundation note writer with an empty body, `source: contacts`, the snapshot id, `title` equal to the file name with one trailing `.md` removed, the current `source_url`, the preserved `imported_at`, no `day`, and no `timezone`. Take the rendered bytes through the newline that ends the rendered closing `---`. Drop the rendered link. Append the preserved body. Write that buffer to the same path.

`imported_at` comes from the first `imported_at:` line in the old front matter. Trim ASCII space around the scalar. If it is double-quoted, unescape `\\`, `\"`, `\n`, `\r`, and `\t`. Pass that text back to the note writer so the new line shows the same timestamp. When the key is absent, use the run's frozen time. Do not read the old `title` key. The file name is the title.

A changed note or a changed display name does not change the body or the file name. The old body still holds the old link and whatever the user wrote under it.

`id` for matching is the first `id:` line in that same block: a double-quoted scalar, unescaped the same way, or a plain scalar with no spaces. A file with no single-line `id` is not a match and is not deleted.

### Deletes

Run only after every create and update has been written.

Delete a markdown file in `contacts/` when its front matter `id` is absent from the snapshot. Leave a markdown file that has no `id`. Leave a name that does not end in `.md`. Leave directories. Do not recurse, so `contacts/nested/gone.md` stays. `remove_file` the matching path directly.

An `Ok` snapshot with zero people still creates `contacts/` when it is missing, deletes markdown files whose ids are now absent, and returns `0`.

## Progress and stdout

Foundation already shows one import line. `total` is the number of people in the snapshot. Before the first write the label is `waiting`. After a write it is the display title defined above. An empty snapshot leaves the label at `waiting`.

Stdout is `contacts=<files>` from the returned count. A permission or fetch error prints `contacts failed: <reason>` and exits `2`. This module does not format either line.

## Dependencies

macOS target only:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
objc2-contacts = "0.3.2"
```

Also depend directly on `objc2`, `objc2-foundation`, and `block2` at the versions `objc2-contacts` 0.3.2 uses, because the fetch code names those crates. Default features of `objc2-contacts` cover `CNContactStore`, `CNContactFetchRequest`, `CNContactFormatter`, `CNLabeledValue`, `CNPhoneNumber`, and `CNPostalAddress`. The enumerate callback is a `block2` block. Linux `cargo test` does not compile these crates into the test.

## Fixtures

JSON objects under `tests/fixtures/contacts/`. The loader builds a `ContactRecord` through `display_title`, `source_url`, `format_address`, `format_birthday`, list sorting, `render_extra`, and the note-body rule.

```json
{
  "identifier": "00000000-0000-0000-0000-000000000001",
  "contact_type": "person",
  "display_name": "Ada Lovelace",
  "organization": "Analytical Engines",
  "job_title": "Mathematician",
  "birthday": { "year": 1815, "month": 12, "day": 10 },
  "phones": [{ "label": "mobile", "value": "+15551212" }],
  "emails": [{ "label": "home", "value": "ada@example.com" }],
  "addresses": [{
    "label": "work",
    "street": "123 Main St",
    "sub_locality": "",
    "city": "London",
    "sub_administrative_area": "",
    "state": "",
    "postal_code": "",
    "country": ""
  }],
  "urls": [{ "label": "homepage", "value": "https://example.com" }],
  "note": "Note text."
}
```

`contact_type` is `person` or `organization`. `birthday` is `null` or an object whose `year` may be `null`. `note` may be `null`.

| File | What it locks |
| --- | --- |
| `tests/fixtures/contacts/ada.json` | The full person above. |
| `tests/fixtures/contacts/company.json` | `contact_type` `organization`, empty `display_name`, `organization` `Analytical Engines`, no note. |
| `tests/fixtures/contacts/birthday-no-year.json` | `birthday.year` null, month `12`, day `10`. |
| `tests/fixtures/contacts/unsorted.json` | Phones labeled `work` then `home`, and two `mobile` values `+2` then `+1`. An email value `Ada@Example.com`. Empty `organization` and empty `job_title`. |

## Tests

`#[cfg(test)]` in `src/sources/contacts.rs`, plus the fixture files. `CARGO_MANIFEST_DIR` locates the fixtures. Temp dirs are `std::env::temp_dir`. Nothing in this list calls Contacts, EventKit, or `chat.db`.

- `fixture_ada_front_matter_and_body`. `ada.json` renders the extra keys in the order above, with the note absent from the YAML. A new file's body is the Open in Contacts link, a blank line, and `Note text.`
- `lists_sorted_and_values_unchanged`. `unsorted.json` emits `home` before `work`, and `mobile` / `+1` before `mobile` / `+2`. The email value stays `Ada@Example.com`. Empty `organization` and `job_title` keys are absent.
- `omit_empty_note`. A record with a whitespace-only note has an empty `note_body`, and the new file ends at the link line.
- `source_url_percent_encodes_identifier`. Identifier `ABC DEF/1` becomes `addressbook://ABC%20DEF%2F1`.
- `import_create`. A missing `contacts/` is created. The Ada file is named `Ada Lovelace.md`. `imported_at` is the frozen string passed in. `title` is `Ada Lovelace`. The returned count is `1`. No inbox directory and no `state.json` are created.
- `import_update_keeps_body_filename_and_imported_at`. Existing file `Custom Name.md` with `imported_at: 2020-01-02T03:04:05-07:00` and body bytes `edited body` (no trailing newline). The record's note is `a different note` and its display name is `Renamed`. After the write the path is still `Custom Name.md`, `title` is `Custom Name`, `imported_at` is `2020-01-02T03:04:05-07:00`, and the bytes after the new closing `---\n` are exactly `edited body`. Front matter has the new phones and no `day` or `timezone`. The directory contains no other file.
- `duplicate_ids_update_utf8_first_path`. `a.md` and `z.md` both carry the same id. Only `a.md` changes. `z.md` is byte-identical, including its old front matter.
- `delete_missing_ids_keeps_files_without_id`. Snapshot contains one id. That file is updated. A markdown file with a different id is removed. A markdown file with no `id` line stays. `notes.txt` stays. `nested/gone.md` stays. The returned count does not include the deletion.
- `company_record_uses_organization_name`. `company.json` creates `Analytical Engines.md`. The progress label observed for that write is `Analytical Engines`.
- `birthday_without_year`. `birthday-no-year.json` emits `birthday: "--12-10"`.
- `permission_failure_writes_nothing`. `import_contacts` with `Err` reason `access denied` does not create `contacts/` when it is absent. When `contacts/Ada.md`, `inbox/old`, and `.pkmagent/state.json` already exist, all three are byte-identical afterward and `Ada.md` is not deleted.
- `contacts_snapshot_requires_macos`, under `cfg(not(target_os = "macos"))`. `fetch_snapshot` returns `Contacts is only available on macOS` and creates no directory.

## Runtime

The live check is foundation's: `import` updates `<vault>/contacts` and creates no inbox folder. A Contacts denial writes nothing, leaves `contacts/` as it is, and exits `2` with `contacts failed: access denied` on stderr.
