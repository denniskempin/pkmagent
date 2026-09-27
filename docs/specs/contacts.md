# Contacts import

Shared vault rules are in [overview.md](overview.md). This file defines the one-time Contacts import.

`pkmagent import` and `pkmagent import --day` do not read Contacts and do not write `<vault>/contacts`.

## Command

`pkmagent import contacts` reads the macOS Contacts store once and writes `<vault>/contacts`. With the default inbox, that directory is `./contacts` in the working directory. The command accepts `--inbox` only to select the vault, the same way other commands do.

The command does not change `last_success_at`. It uses the same import lock as catch-up. It does not take `--since` or `--day`.

Running it again replaces the snapshot. It is not an incremental catch-up and it is not append-only.

## Fetch

Use the Contacts framework, read-only. Include every person record the Contacts app can see, including contacts synced onto the Mac from iCloud, Google, Exchange, and On My Mac. Skip groups and containers.

A contacts permission failure fails the command. Write nothing new, leave the existing `contacts` directory as it is, and exit `2`.

## Files

One Markdown file per person, directly in `<vault>/contacts/`. There are no day folders.

The stable id is the contact identifier from the Contacts framework. The frontmatter `id` is that identifier.

The display title is the contact's display name. A company record uses the organization name. If both are empty, the title is `untitled`. Sanitize the title with the overview rules. The same collision suffix applies.

On a run:

- Create or replace the file for each person. Replacing writes the full current record. It does not append.
- After every person has been written, delete Markdown files in `<vault>/contacts` whose frontmatter `id` is a contact id from a previous snapshot and is not in this snapshot.
- Do not delete a file that has no frontmatter `id`.
- Do not modify the inbox.

Write each file through a sibling `.tmp-` file, then rename it into place. The tool creates `<vault>/contacts` if it is missing.

## Source link

```text
addressbook://<identifier>
```

`<identifier>` is the contact identifier, percent-encoded. This opens that person in Contacts.app. The visible link text is `Open in Contacts`.

## File format

```yaml
---
source: contacts
id: "<contact identifier>"
title: "Ada Lovelace"
source_url: "addressbook://68A46B71-150A-4732-A183-D99EECCE1F18"
organization: "Analytical Engines"
job_title: "Mathematician"
birthday: "1815-12-10"
---
```

Omit `organization`, `job_title`, or `birthday` when the contact has none. `birthday` is `YYYY-MM-DD` when the year is known. When the year is missing, store `--MM-DD`.

```markdown
# Ada Lovelace

[Open in Contacts](addressbook://68A46B71-150A-4732-A183-D99EECCE1F18)

## Phones

- mobile: +15551212

## Emails

- home: ada@example.com

## Addresses

- work: 123 Main St, London

## URLs

- homepage: https://example.com

## Note

Note text.
```

Omit a section when the contact has no values for it. Sort entries within a section by label, then value, using raw UTF-8 order. Phone numbers and emails are stored as the Contacts app shows them. Do not download the contact photo.

## Progress

`pkmagent import contacts` shows one progress bar on stderr. It uses the same bar rules as the overview, with `<source>` set to `contacts` and `<total>` set to the number of people. `<label>` is the display title of the person most recently written, or `waiting` before the first write.

```text
contacts 2/40 [=>                  ] Ada Lovelace
```

Draw it only when stderr is a terminal. It does not appear beside the Gmail, Messages, and Calendar bars. Stdout prints one line when the snapshot finishes: `contacts=<files>`. `<files>` is the number of contact files written.

## Edge cases

- Catch-up never creates or updates `<vault>/contacts`.
- A second `import contacts` rewrites every current person and removes snapshot files for people who are no longer in Contacts.
- A file in `contacts/` with no frontmatter `id` stays in place across that refresh.
- The catch-up cursor is unchanged whether the contacts command succeeds or fails.
