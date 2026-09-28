# Contacts

See [overview.md](overview.md). Gmail, Messages, and Calendar read `<vault>/contacts` for wiki links. Only `import contacts` writes that directory.

## Fetch

Use the Contacts framework, read-only. Skip groups and containers. A permission failure fails the category, leaves existing files as they are, and exits `2`.

The instant for collect is the contact's modification time. A contact with no modification time is skipped by collect and still included by import.

Import includes every person Contacts can see. Collect includes people whose modification time falls in the slice.

## Import

`pkmagent import contacts` writes `<vault>/contacts/`. One file per person. The stable id is the Contacts framework identifier. Create the directory if it is missing.

- No file for that id: create one. The display title is the display name, or the organization name for a company record, or `untitled`. The body is the note.
- A file for that id: replace the front matter with the current record. Leave the body bytes unchanged, including when the note or the display name changed. Do not rename the file. `title` stays the existing filename. `imported_at` stays the existing value.
- After those writes, delete Markdown files in `contacts/` whose frontmatter `id` is absent from this snapshot.
- Leave a file that has no frontmatter `id`.

## Collect

`pkmagent collect contacts` writes new files in the run folder. It does not update `<vault>/contacts`. The display title is chosen the same way as a new import file. The body is the note.

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

On a new file, after the source link, the body is the contact's note. Omit it when the contact has none. An existing file keeps whatever body it already has.

```markdown
Note text.
```

