# Gmail

See [overview.md](overview.md).

## Fetch

Use the Gmail API with the stored readonly credential. One account.

Include every message whose `internalDate` falls in the slice, received or sent, including archived mail. Ignore the `Date` header. Exclude Spam, Trash, and Drafts. `internalDate` is the window instant.

Group by thread id within the output directory. Oldest `internalDate` first. Ties break by Gmail message id, ascending.

## Files

The stable id is the thread id.

The display title is the subject of the chronologically last message in that file, keeping `Re:` and `Fwd:`. An empty subject is `(no subject)`.

## Source link

`https://mail.google.com/mail/?authuser=<account>#all/<thread id>`

`<account>` is the authorized address, percent-encoded. The thread id is not encoded.

## Message body

Prefer the HTML body, converted to Markdown. Keep paragraphs, breaks, headings, lists, emphasis, links, and quotes. Render `blockquote` and `div.gmail_quote` as quotes. Drop `script` and `style`. A remote image becomes `![alt](src)`. Leave a `cid:` image out of the body and list it as an attachment.

If there is no HTML, or the conversion is only whitespace, use `text/plain` when it contains text. Otherwise the section body is empty. Keep quoted history.

## Extra front matter

```yaml
account: name@gmail.com
participants:
  - name: "Ada Lovelace"
    email: ada@example.com
```

`participants` is From, To, Cc, and Bcc on the messages in the file, unique by trimmed email, case-insensitive, sorted by email. `name` is the contact title when the email matches, otherwise the header display name. Two display names for one email use the first in that order.

## Message sections

```markdown
## 2026-09-26 08:14:03 -0700 — [[Ada Lovelace]] <ada@example.com>

- Message-Id: 18c2f0a1b2c3d4e5
- Subject: Quarterly plan
- To: you@example.com
- Cc: [[Ada Lovelace]] <ada@example.com>

Body of the message.
```

`Message-Id` is the Gmail API message id. Every section has a `Subject` line. An empty subject is `(no subject)`. Omit empty `Cc` and `Bcc`. Attachments follow the overview.
