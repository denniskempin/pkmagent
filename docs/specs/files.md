# Files

See [overview.md](overview.md).

`pkmagent collect files` copies regular files from one local directory into the run folder. The window instant `t` is the file's modification time. The copy is the bytes on disk at copy time. This category does not write a Markdown note.

The source directory is only read. Do not modify, move, or delete anything in it.

## Directory

`<vault>/.pkmagent/files.json` names the directory. The user creates it. Collect does not.

```json
{ "path": "/Users/me/Drop" }
```

The file is one JSON object. The only key is `path`, a string that is not empty and not only whitespace. Do not trim it. A relative path is resolved from the process working directory.

Read this file only when `files` is selected, after the lock is taken and before any fetch.

| Condition | Exit | Stderr |
| --- | --- | --- |
| Missing file | `1` | `files.json is missing` |
| Unreadable, not JSON, any other shape, any other key, or a `path` that is missing, empty, only whitespace, or not a string | `1` | `files.json is invalid` |
| The path overlaps the vault | `1` | `files path overlaps the vault` |

The stderr line is that reason by itself. It is not `YYYY-MM-DD failed files:`. Write nothing and leave every cursor unchanged. These checks run even when the files window is empty.

Overlap is compared by path components after removing `.` and `..`. Resolve a symbolic link only on the configured path itself. Reject when that directory is the vault, the inbox, `<vault>/.pkmagent`, or `<vault>/contacts`, or when either path contains the other.

A missing path is not an overlap. That is a fetch failure on the first civil date: `YYYY-MM-DD failed files: directory is missing`. A path that exists and is not a directory fails the same way with `not a directory`.

## Fetch

Walk the directory recursively. Include every regular file whose modification time falls in the slice. Birth time and status-change time do not qualify. A rename that does not change the modification time is not collected again.

Do not follow a symbolic link. Do not copy a symbolic link, socket, device, or fifo, and do not descend through a symbolic link to a directory. Skipping one is not an error. Do descend into directories whose names start with `.`, and include the regular files there.

List each directory's entries in ascending raw UTF-8 order of the name before descending.

A permission error, or a modification time that cannot be read, fails the category. The reason is `reading <relative path> failed`. The relative path is from the source directory and uses `/`. Do not print file contents.

One relative path is one file in the run. The filesystem has no earlier version to copy.

## Copy

Fetch records the relative path and `t`. Copy the bytes after every selected category has been fetched.

- One or two civil dates: `<run>/files/<relative path>`.
- More than two: `<run>/<civil date of t>/files/<relative path>`.

Create parent directories for a file that is written. Do not create a directory that would be empty. Keep the relative path as the directory entry spelled it. Do not sanitize, truncate, or add a collision suffix. Do not add front matter, a source link, or a wiki link.

Write a new regular file with those bytes. Do not hard-link the source. Do not copy extended attributes, ACLs, or the source modification time.

At copy time:

- The path is gone, or it is no longer a regular file: skip it.
- Its modification time is outside the slice: skip it.
- It cannot be read: the write fails, every cursor stays unchanged, and the reason is `reading <relative path> failed`.

The `<files>` count for this category is the number of files written. A run that writes nothing still has no `files/` directory.

## Cursor

The cursor key is `files`. A first collect has no cursor and requires `--since`, like the other categories. On a normal collect the cursor becomes `window_end` after success, including a run that copied nothing. `--day` copies files whose modification time falls on that civil day and does not move the cursor.

Changing `path` does not clear the cursor. The next collect still starts at that timestamp in the new directory.
