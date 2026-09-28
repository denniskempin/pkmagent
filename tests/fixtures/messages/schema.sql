-- Messages fixture schema. Production reads the same columns from chat.db.
-- date_read exists so tests can show it is not the sent time. Production does not select it.

CREATE TABLE message (
    ROWID INTEGER PRIMARY KEY,
    guid TEXT,
    text TEXT,
    attributedBody BLOB,
    handle_id INTEGER,
    is_from_me INTEGER,
    date INTEGER,
    is_sent INTEGER,
    cache_has_attachments INTEGER,
    associated_message_type INTEGER,
    associated_message_guid TEXT,
    associated_message_emoji TEXT,
    destination_caller_id TEXT,
    account TEXT,
    date_retracted INTEGER,
    item_type INTEGER,
    date_read INTEGER
);

CREATE TABLE handle (
    ROWID INTEGER PRIMARY KEY,
    id TEXT
);

CREATE TABLE chat (
    ROWID INTEGER PRIMARY KEY,
    guid TEXT,
    style INTEGER,
    display_name TEXT,
    last_addressed_handle TEXT,
    account_login TEXT
);

CREATE TABLE chat_message_join (
    chat_id INTEGER,
    message_id INTEGER
);

CREATE TABLE chat_handle_join (
    chat_id INTEGER,
    handle_id INTEGER
);

CREATE TABLE attachment (
    ROWID INTEGER PRIMARY KEY,
    filename TEXT,
    mime_type TEXT,
    transfer_name TEXT,
    is_sticker INTEGER
);

CREATE TABLE message_attachment_join (
    message_id INTEGER,
    attachment_id INTEGER
);
