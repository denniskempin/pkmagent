# Messages import

Shared vault, cursor, publishing, and file-writing rules are in [overview.md](overview.md). This file defines the macOS Messages source.

## Fetch

Read the local Messages store at `~/Library/Messages/chat.db` read-only. This includes iMessage, SMS, and RCS stored there. The tool needs Full Disk Access. A locked database or a permission failure fails that day's Messages fetch.

Include every chat, including group chats and messages the user sent. Include plain text, attachment-only messages, tapback reactions, and stickers as their own timestamped entries. Omit unsent messages. An unsent message that is already in a file stays there.

The day of a message is its sent timestamp in the local timezone, not the read timestamp. That sent timestamp is the instant used for the window.

Group by chat. Sections are oldest first. Ties break by the database message id, ascending.

## Files

The stable id is the chat guid. The frontmatter `id` is that guid. The message id used to skip duplicates is the Messages database message id, stored on the `- Id:` line and compared as an exact string.

A later slice the same day appends to the leftover chat file when it is still in the day's `messages/` directory. If several files share the id, append to the path that sorts first by raw UTF-8 bytes. If that file is gone, the later slice creates a new file that contains only the new messages.

Display title for a new file:

| Chat | Title |
| --- | --- |
| Direct | The other participant's display name if Messages has one, otherwise the phone number or email exactly as stored. A chat with only the user is `Me`. |
| Group | The group display name if it has one. Otherwise the participant labels sorted by raw UTF-8 bytes, joined with `, `. |

The other participant in a direct chat is the participant that is not the user's own Messages account. Phone numbers and emails are not reformatted. A whitespace-only group name counts as no name. Contact names are whatever Messages associates with the handle at import time. Sanitize the title with the overview rules.

## Source link

The link opens Messages.app to the participants in the chat. There is no documented URL for a chat guid, so the link addresses the other participants.

1. The scheme is `sms` when the chat guid starts with `SMS;`. Otherwise the scheme is `imessage`.
2. Take every participant handle that is not the user's own account, in frontmatter order.
3. Percent-encode each handle. Encode `+` as `%2B` and `@` as `%40`. Leave the commas that join handles unencoded.
4. If that list is empty, `source_url` is `messages://`. This opens Messages.app and not a specific thread.
5. Otherwise `source_url` is `<scheme>://<handle>,<handle>,...`.

A direct chat uses one handle: `imessage://%2B15551212`. A group chat joins the other participants: `imessage://%2B15551212,ada%40icloud.com`. The visible link text is `Open in Messages`. Use the chat's full participant list, not only people who sent a message in the slice.

## File format

```yaml
---
source: messages
id: "<chat guid>"
title: "Ada Lovelace"
source_url: "imessage://%2B15551212"
day: YYYY-MM-DD
timezone: America/Los_Angeles
imported_at: 2026-09-27T18:04:11-07:00
participants:
  - name: "Ada Lovelace"
    handle: "+15551212"
  - name: ""
    handle: "me@icloud.com"
    self: true
---
```

On a new file, participants are the chat participants sorted by handle, raw UTF-8 order. The user's own account has `self: true`. Outgoing sender label is `Me`.

```markdown
# Ada Lovelace

[Open in Messages](imessage://%2B15551212)

## 2026-09-27 09:01:00 -0700 — Ada Lovelace

- Id: 48211

See you there.

### Attachments

- photo.jpg (image/jpeg)
```

`Id` is the Messages database message id. Reactions and stickers use the same heading and `Id` line. Omit the attachments section when there are none. An attachment with no filename is `unnamed`. List the media type when the source provides it.

A reaction entry's body is one line:

```text
Reaction: Like to "dinner at 7"
```

The quoted target is the reacted-to message's plain text, whitespace collapsed, truncated to 80 scalar values. If it has no text, use its first attachment filename. If neither exists, use `a message`. Use the database's reaction name when it has one (`Love`, `Like`, `Dislike`, `Laugh`, `Emphasize`, `Question`, or a custom value).

A sticker entry's body is `Sticker: <filename>` or `Sticker: sticker` when there is no filename.

## Edge cases

- A contact rename does not rename the file. A new participant appears as the sender of an appended section.
- An unsent message already in a file stays there. New unsent messages are omitted.
- The same chat on Monday and Tuesday becomes two files. Each file receives only the messages whose timestamps fall on that day.
