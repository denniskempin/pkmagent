# Messages import

See [overview.md](overview.md).

## Fetch

Read `~/Library/Messages/chat.db` read-only. The process needs Full Disk Access. A locked database or a permission failure fails that day's fetch.

Include every chat, including groups and messages the user sent. Include text, attachment-only messages, tapbacks, and stickers, each as its own entry. Omit unsent messages.

The window instant is the sent time, not the read time. Group by chat. Oldest first. Ties break by database message id, ascending.

## Files

The stable id is the chat guid. Skip an entry whose database message id is already on an `- Id:` line in the matched file.

Display title for a new file:

| Chat | Title |
| --- | --- |
| Direct | The contact title when the other participant's handle matches. Otherwise the Messages display name, otherwise the handle as stored. A chat with only the user is `Me`. |
| Group | The group name. If it has none, each participant's contact title, display name, or handle, in that order, sorted by raw UTF-8 and joined with `, `. |

The other participant is anyone who is not the user's own account. A whitespace-only group name counts as no name. Store handles as they appear in the database.

## Source link

There is no URL for a chat guid. The link addresses the other participants, using the full chat, not only people who sent a message in the slice.

1. The scheme is `sms` when the guid starts with `SMS;`, otherwise `imessage`.
2. Take every handle that is not the user's account, in frontmatter order.
3. Percent-encode each handle. Encode `+` as `%2B` and `@` as `%40`. Leave the joining commas unencoded.
4. If none remain, `source_url` is `messages://`.
5. Otherwise `source_url` is `<scheme>://<handle>,<handle>,...`.

A direct chat is `imessage://%2B15551212`. A group chat is `imessage://%2B15551212,ada%40icloud.com`.

## Extra front matter

```yaml
participants:
  - name: "Ada Lovelace"
    handle: "+15551212"
  - name: ""
    handle: "me@icloud.com"
    self: true
```

Participants on a new file are the chat members sorted by handle, raw UTF-8. The user's account has `self: true`. `name` is the contact title when the handle matches, otherwise the Messages display name, otherwise empty.

## Message sections

```markdown
## 2026-09-27 09:01:00 -0700 — [[Ada Lovelace]]

- Id: 48211

See you there.
```

`Id` is the database message id. The outgoing sender is `Me`. Attachments follow the overview.

A reaction is one line. The quote is the target message's plain text, whitespace collapsed, truncated to 80 scalar values; else its first attachment filename; else `a message`. Use the stored reaction name (`Love`, `Like`, `Dislike`, `Laugh`, `Emphasize`, `Question`, or a custom value).

```text
Reaction: Like to "dinner at 7"
```

A sticker is `Sticker: <filename>`, or `Sticker: sticker` when it has no filename.
