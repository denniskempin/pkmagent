-- Fixture rows for plan 003. Times are Apple absolute time.
-- Chat 1 stores destination_caller_id on every row so self is me@icloud.com, not the account.

INSERT INTO handle (ROWID, id) VALUES
    (1, '+15551212'),
    (2, 'ada@icloud.com'),
    (3, '+15559876'),
    (4, '+15550000');

INSERT INTO chat (ROWID, guid, style, display_name, last_addressed_handle, account_login) VALUES
    (1, 'iMessage;-;+15551212', 45, NULL, NULL, NULL),
    (2, 'iMessage;+;chat555', 43, '   ', NULL, NULL),
    (3, 'SMS;-;+15559876', 45, NULL, NULL, NULL),
    (4, 'iMessage;-;self', 45, NULL, NULL, NULL),
    (5, 'iMessage;-;+15550000', 45, 'Sam', NULL, NULL),
    (6, 'iMessage;+;chat777', 43, 'Book club', 'me@icloud.com', NULL);

INSERT INTO chat_handle_join (chat_id, handle_id) VALUES
    (1, 1),
    (2, 1),
    (2, 2),
    (3, 3),
    (5, 4),
    (6, 1),
    (6, 2);

INSERT INTO message (
    ROWID, guid, text, attributedBody, handle_id, is_from_me, date, is_sent,
    cache_has_attachments, associated_message_type, associated_message_guid,
    associated_message_emoji, destination_caller_id, account, date_retracted,
    item_type, date_read
) VALUES
    (100, 'G100', 'See you there.', X'6E6F742D612D73747265616D', 1, 0, 812217660, 1,
        0, 0, NULL, NULL, 'me@icloud.com', 'E:other@icloud.com', 0, 0, 812300000),
    (101, NULL, 'On my way', NULL, NULL, 1, 812217720000000000, 1,
        0, 0, NULL, NULL, 'me@icloud.com', 'E:other@icloud.com', 0, 0, NULL),
    (102, NULL, NULL, NULL, NULL, 1, 812217840, 1,
        0, 2001, 'p:0/G115', NULL, 'me@icloud.com', 'E:other@icloud.com', 0, 0, NULL),
    (103, 'G103', NULL, NULL, 1, 0, 812217900, 1,
        0, 0, NULL, NULL, 'me@icloud.com', 'E:other@icloud.com', 0, 0, NULL),
    (104, NULL, NULL, NULL, 1, 0, 812217960, 1,
        1, 0, NULL, NULL, 'me@icloud.com', 'E:other@icloud.com', 0, 0, NULL),
    (105, NULL, NULL, NULL, 1, 0, 812218020, 1,
        1, 0, NULL, NULL, 'me@icloud.com', 'E:other@icloud.com', 0, 0, NULL),
    (106, NULL, NULL, NULL, NULL, 1, 812218080, 1,
        0, 2000, 'p:0/G103', NULL, 'me@icloud.com', 'E:other@icloud.com', 0, 0, NULL),
    (107, NULL, NULL, NULL, 1, 0, 812218140, 1,
        0, 2003, 'p:0/does-not-exist', NULL, 'me@icloud.com', 'E:other@icloud.com', 0, 0, NULL),
    (108, NULL, 'secret unsent body', NULL, NULL, 1, 812218200, 0,
        0, 0, NULL, NULL, 'me@icloud.com', 'E:other@icloud.com', 0, 0, NULL),
    (109, NULL, 'secret retracted body', NULL, 1, 0, 812218260, 1,
        0, 0, NULL, NULL, 'me@icloud.com', 'E:other@icloud.com', 1, 0, NULL),
    (110, NULL, 'too early', NULL, 1, 0, 812142000, 1,
        0, 0, NULL, NULL, 'me@icloud.com', 'E:other@icloud.com', 0, 0, 812217660),
    (111, NULL, 'Late', NULL, 1, 0, 812262600, 1,
        0, 0, NULL, NULL, 'me@icloud.com', 'E:other@icloud.com', 0, 0, NULL),
    (112, NULL, NULL, NULL, NULL, 1, 812218320, 1,
        0, 2004, 'p:0/G112', NULL, 'me@icloud.com', 'E:other@icloud.com', 0, 0, NULL),
    (113, NULL, NULL, NULL, 1, 0, 812218680, 1,
        0, 2006, 'p:0/G100', 'Cheer', 'me@icloud.com', 'E:other@icloud.com', 0, 0, NULL),
    (115, 'G115', 'dinner ' || char(10) || char(10) || ' at   7', NULL, 1, 0, 812142300, 1,
        0, 0, NULL, NULL, 'me@icloud.com', 'E:other@icloud.com', 0, 0, NULL),
    (116, 'G112', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaéZ', NULL, 1, 0, 812142360, 1,
        0, 0, NULL, NULL, 'me@icloud.com', 'E:other@icloud.com', 0, 0, NULL),
    (200, NULL, 'Ping', NULL, 3, 0, 812218380, 1,
        0, 0, NULL, NULL, 'me@icloud.com', NULL, 0, 0, NULL),
    (300, NULL, 'Note to self', NULL, NULL, 1, 812218440, 1,
        0, 0, NULL, NULL, NULL, 'E:me@icloud.com', 0, 0, NULL),
    (400, NULL, 'Hey', NULL, 4, 0, 812218500, 1,
        0, 0, NULL, NULL, 'me@icloud.com', NULL, 0, 0, NULL),
    (500, NULL, 'Group hello', NULL, 1, 0, 812218560, 1,
        0, 0, NULL, NULL, 'me@icloud.com', NULL, 0, 0, NULL),
    (600, NULL, 'Tonight', NULL, 1, 0, 812218620, 1,
        0, 0, NULL, NULL, NULL, NULL, 0, 0, NULL),
    (700, NULL, 'After midnight', NULL, 1, 0, 812273400, 1,
        0, 0, NULL, NULL, 'me@icloud.com', 'E:other@icloud.com', 0, 0, NULL);

INSERT INTO chat_message_join (chat_id, message_id) VALUES
    (1, 100), (1, 101), (1, 102), (1, 103), (1, 104), (1, 105),
    (1, 106), (1, 107), (1, 108), (1, 109), (1, 110), (1, 111),
    (1, 112), (1, 113), (1, 115), (1, 116), (1, 700),
    (3, 200),
    (4, 300),
    (5, 400),
    (2, 500),
    (6, 600);

INSERT INTO attachment (ROWID, filename, mime_type, transfer_name, is_sticker) VALUES
    (1, '/Users/ada/Library/Messages/Attachments/aa/bb/pic-on-disk.jpg', 'image/jpeg', 'pic.jpg', 0),
    (2, '/Users/ada/Library/Messages/Attachments/aa/bb/heart.png', 'image/png', 'heart.png', 1),
    (3, NULL, NULL, NULL, 1);

INSERT INTO message_attachment_join (message_id, attachment_id) VALUES
    (103, 1),
    (104, 2),
    (105, 3);
