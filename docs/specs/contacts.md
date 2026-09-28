# Contacts import

See [overview.md](overview.md). Catch-up does not read or write `<vault>/contacts`.

## Command

`pkmagent import contacts` reads the macOS Contacts store and writes `<vault>/contacts`. It does not change `last_success_at` and uses the same import lock. It takes no `--since` or `--day`.

Running it again replaces the snapshot.

## Fetch

Use the Contacts framework, read-only. Include every person Contacts can see. Skip groups and containers.

A permission failure writes nothing, leaves `contacts/` as it is, and exits `2`.

## Files

One file per person, directly in `<vault>/contacts/`. The stable id is the Contacts framework identifier.

The display title is the display name, or the organization name for a company record, or `untitled`.

- Write each current person as a full record, replacing any previous file for that id.
- After those writes, delete Markdown files in `contacts/` whose frontmatter `id` is absent from this snapshot.
- Leave a file that has no frontmatter `id`.
- Write through a sibling `.tmp-` file, then rename. Create `contacts/` if it is missing.

## Source link

`addressbook://<identifier>`, with the identifier percent-encoded. This opens that person in Contacts.app.

## Extra front matter

Phones, emails, addresses, and URLs go here. The note does not.

```yaml
organization: "Analytical Engines"
job_title: "Mathematician"
birthday: "1815-12-10"
phones:
  - label: mobile
    value: "+15551212"
emails:
  - label: home
    value: ada@example.com
addresses:
  - label: work
    value: "123 Main St, London"
urls:
  - label: homepage
    value: https://example.com
```

`birthday` is `YYYY-MM-DD`, or `--MM-DD` when the year is missing. Sort each list by label, then value, raw UTF-8. Store phone numbers and emails as Contacts shows them.

## Body

After the source link, the body is the contact's note. Omit it when the contact has none.

```markdown
Note text.
```

## Progress

One status line, same shape as the overview, with source `contacts` and `<total>` equal to the number of people. The label is the display title most recently written, or `waiting`. Stdout on success: `contacts=<files>`, the number of files written.
