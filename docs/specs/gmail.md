# Gmail import

Shared vault, cursor, publishing, and file-writing rules are in [overview.md](overview.md). This file defines the Gmail source.

## Fetch

Use the Gmail API with the stored readonly credential. There is one account, the account that `pkmagent auth gmail` stored. The scope and credential paths are in the overview.

Include every message whose Gmail `internalDate` falls in the slice, whether it was received or sent, including archived mail. Ignore the `Date` header. Exclude Spam, Trash, and Drafts. The instant used for the window is `internalDate`.

Group the slice's messages by Gmail thread id. Sections for that thread are oldest `internalDate` first. Ties break by Gmail message id, ascending. Quoted history inside a body stays. The tool does not fetch messages outside the slice, including earlier messages from the same day that are already in a leftover file.

If Gmail auth is missing or expired, that day's Gmail fetch fails and the day is not written.

## Files

The stable id is the Gmail thread id. The frontmatter `id` is that thread id. The message id used to skip duplicates is the Gmail API message id, stored on the `- Message-Id:` line and compared as an exact string.

A thread can have a separate file on each day it has messages. Sent and received messages in the same thread and the same slice go into one file. A later slice the same day appends to that file when it is still in the day's `gmail/` directory. If several files share the id, append to the path that sorts first by raw UTF-8 bytes. If that file is gone, the later slice creates a new file that contains only the new messages.

The display title for a new file is the subject of the chronologically last message included in that new file, including `Re:` and `Fwd:`. An empty subject becomes `(no subject)`. Sanitize the title with the overview rules.

## Source link

`source_url` is `https://mail.google.com/mail/?authuser=<account>#all/<thread id>`. `<account>` is the authorized Gmail address, percent-encoded as a query parameter. `<thread id>` is the Gmail API thread id and is not encoded. The note layout for that URL is in [overview.md](overview.md).

## Message body

When the message has an HTML body, convert that HTML to Markdown and use the Markdown as the section body. Keep paragraphs, line breaks, headings, lists, emphasis, links, and block quotes. Render a link as `[label](url)`, using the url as the label when the label is empty. Render `blockquote` and `div.gmail_quote` as Markdown quotes. Drop `script` and `style`. Do not download attachments or remote images. An `img` whose `src` is `http` or `https` becomes `![alt](src)`, with empty alt when the image has none. Leave a `cid:` image out of the body; its file still appears in the attachment list.

If the message has no HTML, or the conversion is only whitespace and a `text/plain` body exists, use `text/plain` as the section body. If neither body yields text, the section body is empty. Quoted history stays in whichever body is used.

## Extra front matter

After the shared fields:

```yaml
account: name@gmail.com
participants:
  - name: "Ada Lovelace"
    email: ada@example.com
```

`participants` is the union of From, To, Cc, and Bcc on the messages in the new file, unique by email address, sorted by email. `name` is the contact title when the email matches, otherwise the header display name, or empty when that is unknown. Later participants appear in appended sections and are not added to this list.

## Message sections

Each message is a `##` heading with the local timestamp and the sender, then these lines:

```markdown
## 2026-09-26 08:14:03 -0700 — [[Ada Lovelace]] <ada@example.com>

- Message-Id: 18c2f0a1b2c3d4e5
- Subject: Quarterly plan
- To: you@example.com
- Cc: [[Ada Lovelace]] <ada@example.com>

Body of the message.
```

`Message-Id` is the Gmail API message id. Omit `Cc` and `Bcc` when empty. Order messages oldest first. A changed subject on a later message is a new `Subject` line on that message's section. Attachments follow the overview.

## Edge cases

- Mail that arrived during the slice and was later archived is included. Spam, Trash, and Drafts are not.
- HTML mail is stored as Markdown. A message whose HTML conversion is empty uses its plain-text body. A message with only plain text stays plain text.
- An `internalDate` is included only when it falls inside the slice. A timestamp after the frozen end waits for the next run.
- The same thread on Monday and Tuesday becomes two files. Each file receives only the messages whose timestamps fall on that day.
- A subject change does not rename the file. The new subject is the `Subject` line of the appended section.
- A message moved to Trash is not removed from an existing file. A new message that is in Trash is not imported.
