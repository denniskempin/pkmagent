//! Messages. The live `chat.db` open is macOS-only. SQL and note mapping run everywhere.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, FixedOffset, Utc};
use chrono_tz::Tz;
use rusqlite::{Connection, Error, ErrorCode, Row};

use crate::contacts_index::ContactIndex;
use crate::note::yaml_double_quoted;
use crate::sources::{Attachment, CollectSource, CollectedItem, SourceError};
use crate::timeutil::Slice;

const APPLE_EPOCH_OFFSET: i64 = 978_307_200;
const APPLE_NANO_CUTOFF: i64 = 1_000_000_000_000_000;
const NANOS_PER_SECOND: i64 = 1_000_000_000;

pub struct MessagesSource {
    pub zone: Tz,
}

impl CollectSource for MessagesSource {
    fn fetch(
        &self,
        slice: &Slice,
        contacts: &ContactIndex,
    ) -> Result<Vec<CollectedItem>, SourceError> {
        #[cfg(target_os = "macos")]
        {
            let conn = open_live_database()?;
            return fetch_messages(&conn, slice, contacts, &self.zone);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (slice, contacts, self.zone);
            Err(SourceError::new("Messages is only available on macOS"))
        }
    }
}

/// Query and map messages. Does not open `~/Library/Messages/chat.db`.
pub fn fetch_messages(
    conn: &Connection,
    slice: &Slice,
    contacts: &ContactIndex,
    zone: &Tz,
) -> Result<Vec<CollectedItem>, SourceError> {
    let columns = message_columns(conn)?;
    let (candidates, chats) = query_candidates(conn, &columns, slice, zone)?;
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let chat_ids = unique_ids(candidates.iter().map(|message| message.chat_id));
    let message_ids = unique_ids(candidates.iter().map(|message| message.rowid));
    let mut attachments = load_attachments(conn, &message_ids)?;
    let targets = load_targets(conn, &reaction_guids(&candidates))?;
    merge_target_attachments(conn, &targets, &mut attachments)?;
    let members = load_members(conn, &chat_ids)?;
    let self_rows = load_self_rows(conn, &chat_ids)?;

    let mut views = HashMap::new();
    for (chat_id, meta) in &chats {
        let view = build_chat_view(
            meta,
            members.get(chat_id).map(Vec::as_slice).unwrap_or(&[]),
            self_rows.get(chat_id).map(Vec::as_slice).unwrap_or(&[]),
            contacts,
        );
        views.insert(*chat_id, view);
    }

    let mut items = Vec::new();
    for message in &candidates {
        let rows = attachments
            .get(&message.rowid)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let Some(content) = render_content(message, rows, &targets, &attachments) else {
            continue;
        };
        let view = &views[&message.chat_id];
        let sender = sender_label(message, view, contacts);
        let rowid = message.rowid;
        let text = match &content {
            SectionText::Blank => None,
            SectionText::Line(line) => Some(line.as_str()),
        };
        items.push(CollectedItem {
            stable_id: view.stable_id.clone(),
            display_title: view.display_title.clone(),
            empty_title_fallback: view.empty_title_fallback.clone(),
            source_url: view.source_url.clone(),
            t: message.t,
            tie_break: format!("{rowid:020}"),
            file_day: message.t.date_naive(),
            extra_front_matter: view.extra_front_matter.clone(),
            body: section_body(message.t, &sender, rowid, text),
            attachments: collected_attachments(rows),
        });
    }
    Ok(items)
}

#[cfg(target_os = "macos")]
fn open_live_database() -> Result<Connection, SourceError> {
    use std::os::unix::ffi::OsStrExt;
    use std::time::Duration;

    use rusqlite::OpenFlags;

    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .ok_or_else(|| {
            SourceError::new("cannot open Messages database: home directory is unknown")
        })?;
    let path = std::path::Path::new(&home).join("Library/Messages/chat.db");
    let uri = sqlite_file_uri(path.as_os_str().as_bytes());
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI;
    let conn = match Connection::open_with_flags(&uri, flags) {
        Ok(conn) => conn,
        Err(err) => {
            let denied = path_permission_denied(&path);
            return Err(SourceError::new(open_failure_reason(&err, denied)));
        }
    };
    // rusqlite installs a 5s busy handler during open. Clear it before any SQL so a
    // contended lock fails immediately, then apply the read-only pragmas.
    conn.busy_timeout(Duration::ZERO).map_err(read_failure)?;
    conn.execute_batch("PRAGMA query_only = ON; PRAGMA busy_timeout = 0;")
        .map_err(read_failure)?;
    Ok(conn)
}

#[cfg(target_os = "macos")]
fn path_permission_denied(path: &std::path::Path) -> bool {
    match std::fs::File::open(path) {
        Ok(_) => false,
        Err(err) => err.kind() == std::io::ErrorKind::PermissionDenied,
    }
}

fn sqlite_file_uri(path: &[u8]) -> String {
    let mut uri = String::from("file:");
    for &byte in path {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.') {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri.push_str("?mode=ro");
    uri
}

fn open_failure_reason(err: &Error, os_permission_denied: bool) -> String {
    if sqlite_is_locked(err) {
        return "Messages database is locked".to_string();
    }
    if sqlite_is_unreadable(err, os_permission_denied) {
        return "Messages database is not readable (Full Disk Access required)".to_string();
    }
    format!("cannot open Messages database: {}", sqlite_errmsg(err))
}

fn sqlite_is_unreadable(err: &Error, os_permission_denied: bool) -> bool {
    let code_matches = matches!(
        err.sqlite_error_code(),
        Some(ErrorCode::CannotOpen | ErrorCode::AuthorizationForStatementDenied)
    );
    if !code_matches {
        return false;
    }
    if os_permission_denied {
        return true;
    }
    let message = sqlite_errmsg(err).to_ascii_lowercase();
    message.contains("operation not permitted") || message.contains("permission denied")
}

fn sqlite_is_locked(err: &Error) -> bool {
    matches!(
        err.sqlite_error_code(),
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

fn sqlite_errmsg(err: &Error) -> String {
    match err {
        Error::SqliteFailure(_, Some(message)) => message.clone(),
        other => other.to_string(),
    }
}

fn read_failure(err: Error) -> SourceError {
    SourceError::new(if sqlite_is_locked(&err) {
        "Messages database is locked".to_string()
    } else {
        format!("cannot read Messages database: {}", sqlite_errmsg(&err))
    })
}

fn message_columns(conn: &Connection) -> Result<HashSet<String>, SourceError> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(message)")
        .map_err(read_failure)?;
    let mut names = HashSet::new();
    let mut rows = stmt.query([]).map_err(read_failure)?;
    while let Some(row) = rows.next().map_err(read_failure)? {
        let name: String = row.get(1).map_err(read_failure)?;
        names.insert(name);
    }
    Ok(names)
}

fn has_column(columns: &HashSet<String>, name: &str) -> bool {
    columns
        .iter()
        .any(|column| column.eq_ignore_ascii_case(name))
}

fn message_sql(columns: &HashSet<String>) -> String {
    let emoji = if has_column(columns, "associated_message_emoji") {
        "m.associated_message_emoji AS associated_message_emoji"
    } else {
        "NULL AS associated_message_emoji"
    };
    let retracted = if has_column(columns, "date_retracted") {
        "m.date_retracted AS date_retracted"
    } else {
        "0 AS date_retracted"
    };
    format!(
        "SELECT
            m.ROWID AS message_rowid,
            m.guid AS message_guid,
            m.text AS text,
            m.attributedBody AS attributed_body,
            m.handle_id AS handle_id,
            m.is_from_me AS is_from_me,
            m.date AS sent_date,
            m.is_sent AS is_sent,
            m.cache_has_attachments AS cache_has_attachments,
            m.associated_message_type AS associated_message_type,
            m.associated_message_guid AS associated_message_guid,
            {emoji},
            m.destination_caller_id AS destination_caller_id,
            m.account AS account,
            {retracted},
            m.item_type AS item_type,
            c.ROWID AS chat_rowid,
            c.guid AS chat_guid,
            c.style AS chat_style,
            c.display_name AS display_name,
            c.last_addressed_handle AS last_addressed_handle,
            c.account_login AS account_login,
            h.id AS sender_handle
         FROM message AS m
         JOIN chat_message_join AS cmj ON cmj.message_id = m.ROWID
         JOIN chat AS c ON c.ROWID = cmj.chat_id
         LEFT JOIN handle AS h ON h.ROWID = m.handle_id
         WHERE (m.date > 1000000000000000 AND m.date >= :nano_start AND m.date < :nano_end)
            OR (m.date <= 1000000000000000 AND m.date >= :sec_start AND m.date < :sec_end)
         ORDER BY m.ROWID, c.ROWID"
    )
}

fn query_candidates(
    conn: &Connection,
    columns: &HashSet<String>,
    slice: &Slice,
    zone: &Tz,
) -> Result<(Vec<RawMessage>, HashMap<i64, ChatMeta>), SourceError> {
    let (nano_start, nano_end, sec_start, sec_end) = apple_slice_bounds(slice);
    let mut stmt = conn.prepare(&message_sql(columns)).map_err(read_failure)?;
    let mut rows = stmt
        .query(rusqlite::named_params! {
            ":nano_start": nano_start,
            ":nano_end": nano_end,
            ":sec_start": sec_start,
            ":sec_end": sec_end,
        })
        .map_err(read_failure)?;
    let mut candidates = Vec::new();
    let mut chats = HashMap::new();
    while let Some(row) = rows.next().map_err(read_failure)? {
        let Some((message, meta)) = read_candidate(row, slice, zone).map_err(read_failure)? else {
            continue;
        };
        chats.entry(message.chat_id).or_insert(meta);
        candidates.push(message);
    }
    Ok((candidates, chats))
}

fn read_candidate(
    row: &Row<'_>,
    slice: &Slice,
    zone: &Tz,
) -> rusqlite::Result<Option<(RawMessage, ChatMeta)>> {
    // Selected because the schema lists them. Self handles come from every message
    // in the chat, and the sender handle comes from the join.
    let _message_guid: Option<String> = row.get("message_guid")?;
    let _handle_id: Option<i64> = row.get("handle_id")?;
    let _cache_has_attachments: Option<i64> = row.get("cache_has_attachments")?;
    let _destination_caller_id: Option<String> = row.get("destination_caller_id")?;
    let _account: Option<String> = row.get("account")?;

    let Some(sent) = row.get::<_, Option<i64>>("sent_date")? else {
        return Ok(None);
    };
    let Some(t) = apple_to_local(sent, zone) else {
        return Ok(None);
    };
    if t < slice.start || t >= slice.end {
        return Ok(None);
    }

    let message = RawMessage {
        rowid: row.get("message_rowid")?,
        text: row.get("text")?,
        attributed_body: row.get("attributed_body")?,
        is_from_me: row.get::<_, Option<i64>>("is_from_me")?.unwrap_or(0),
        is_sent: row.get("is_sent")?,
        associated_message_type: row.get("associated_message_type")?,
        associated_message_guid: row.get("associated_message_guid")?,
        associated_message_emoji: row.get("associated_message_emoji")?,
        date_retracted: row.get("date_retracted")?,
        item_type: row.get("item_type")?,
        chat_id: row.get("chat_rowid")?,
        sender_handle: row.get("sender_handle")?,
        t,
    };
    if row_excluded(&message) {
        return Ok(None);
    }
    let meta = ChatMeta {
        guid: row
            .get::<_, Option<String>>("chat_guid")?
            .unwrap_or_default(),
        style: row.get::<_, Option<i64>>("chat_style")?.unwrap_or(0),
        display_name: row.get("display_name")?,
        last_addressed_handle: row.get("last_addressed_handle")?,
        account_login: row.get("account_login")?,
    };
    Ok(Some((message, meta)))
}

struct RawMessage {
    rowid: i64,
    text: Option<String>,
    attributed_body: Option<Vec<u8>>,
    is_from_me: i64,
    is_sent: Option<i64>,
    associated_message_type: Option<i64>,
    associated_message_guid: Option<String>,
    associated_message_emoji: Option<String>,
    date_retracted: Option<i64>,
    item_type: Option<i64>,
    chat_id: i64,
    sender_handle: Option<String>,
    t: DateTime<FixedOffset>,
}

struct ChatMeta {
    guid: String,
    style: i64,
    display_name: Option<String>,
    last_addressed_handle: Option<String>,
    account_login: Option<String>,
}

struct StoredAttachment {
    rowid: i64,
    message_id: i64,
    filename: Option<String>,
    mime_type: Option<String>,
    transfer_name: Option<String>,
    is_sticker: Option<i64>,
}

struct TargetMessage {
    rowid: i64,
    text: Option<String>,
    attributed_body: Option<Vec<u8>>,
}

struct Person {
    handle: String,
    is_self: bool,
}

struct ChatView {
    stable_id: String,
    display_title: String,
    empty_title_fallback: String,
    source_url: String,
    extra_front_matter: String,
    direct_name: Option<String>,
    is_direct: bool,
}

enum SectionText {
    Blank,
    Line(String),
}

fn row_excluded(message: &RawMessage) -> bool {
    if message.is_from_me != 0 && message.is_sent == Some(0) {
        return true;
    }
    if matches!(message.date_retracted, Some(value) if value != 0) {
        return true;
    }
    if matches!(message.item_type, Some(value) if value != 0) {
        return true;
    }
    matches!(message.associated_message_type, Some(value) if (3000..4000).contains(&value))
}

fn apple_parts(date: i64) -> (i64, u32) {
    if date > APPLE_NANO_CUTOFF {
        let seconds = date.div_euclid(NANOS_PER_SECOND);
        let nanos = date.rem_euclid(NANOS_PER_SECOND);
        (seconds, nanos as u32)
    } else {
        (date, 0)
    }
}

fn apple_to_utc(date: i64) -> Option<DateTime<Utc>> {
    if date == 0 {
        return None;
    }
    let (seconds, nanos) = apple_parts(date);
    let unix = seconds.checked_add(APPLE_EPOCH_OFFSET)?;
    DateTime::from_timestamp(unix, nanos)
}

fn apple_to_local(date: i64, zone: &Tz) -> Option<DateTime<FixedOffset>> {
    Some(apple_to_utc(date)?.with_timezone(zone).fixed_offset())
}

fn apple_slice_bounds(slice: &Slice) -> (i64, i64, i64, i64) {
    let sec_start = slice.start.timestamp().saturating_sub(APPLE_EPOCH_OFFSET);
    let sec_end = slice.end.timestamp().saturating_sub(APPLE_EPOCH_OFFSET);
    (
        sec_start.saturating_mul(NANOS_PER_SECOND),
        sec_end.saturating_mul(NANOS_PER_SECOND),
        sec_start,
        sec_end,
    )
}

fn plain_text(text: Option<&str>, attributed_body: Option<&[u8]>) -> Option<String> {
    if let Some(text) = text {
        if !text.is_empty() {
            return Some(normalize_newlines(text));
        }
    }
    let body = attributed_body.filter(|body| !body.is_empty())?;
    decode_attributed_body(body).map(|text| normalize_newlines(&text))
}

fn normalize_newlines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn decode_attributed_body(blob: &[u8]) -> Option<String> {
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::runtime::AnyClass;
    use objc2::{msg_send, sel};
    use objc2_foundation::{NSData, NSObject, NSString};

    autoreleasepool(|_| unsafe {
        let data = NSData::dataWithBytes_length(blob.as_ptr().cast(), blob.len());
        let class = AnyClass::get(c"NSUnarchiver")?;
        let decoded: Option<Retained<NSObject>> = msg_send![class, unarchiveObjectWithData: &*data];
        let decoded = decoded?;
        if !decoded.respondsToSelector(sel!(string)) {
            return None;
        }
        let string: Option<Retained<NSString>> = msg_send![&*decoded, string];
        Some(string?.to_string())
    })
}

#[cfg(not(target_os = "macos"))]
fn decode_attributed_body(blob: &[u8]) -> Option<String> {
    let _ = blob;
    None
}

fn reaction_name(kind: Option<i64>, emoji: Option<&str>) -> Option<String> {
    let standard = match kind {
        Some(2000) => Some("Love"),
        Some(2001) => Some("Like"),
        Some(2002) => Some("Dislike"),
        Some(2003) => Some("Laugh"),
        Some(2004) => Some("Emphasize"),
        Some(2005) => Some("Question"),
        _ => None,
    };
    if let Some(name) = standard {
        return Some(name.to_string());
    }
    let emoji = emoji?.trim();
    if emoji.is_empty() {
        None
    } else {
        Some(emoji.to_string())
    }
}

fn reaction_target_guid(raw: &str) -> &str {
    if let Some(index) = raw.rfind('/') {
        &raw[index + 1..]
    } else if let Some(index) = raw.rfind(':') {
        &raw[index + 1..]
    } else {
        raw
    }
}

fn reaction_guids(messages: &[RawMessage]) -> Vec<String> {
    let mut guids = Vec::new();
    let mut seen = HashSet::new();
    for message in messages {
        if reaction_name(
            message.associated_message_type,
            message.associated_message_emoji.as_deref(),
        )
        .is_none()
        {
            continue;
        }
        let Some(raw) = message.associated_message_guid.as_deref() else {
            continue;
        };
        let guid = reaction_target_guid(raw);
        if guid.is_empty() || !seen.insert(guid.to_string()) {
            continue;
        }
        guids.push(guid.to_string());
    }
    guids
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_scalars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn display_filename(transfer_name: Option<&str>, filename: Option<&str>) -> Option<String> {
    if let Some(name) = transfer_name.map(str::trim).filter(|name| !name.is_empty()) {
        return Some(name.to_string());
    }
    let path = filename?.trim();
    if path.is_empty() {
        return None;
    }
    let base = path.rsplit('/').next().unwrap_or(path);
    if base.is_empty() {
        None
    } else {
        Some(base.to_string())
    }
}

fn render_content(
    message: &RawMessage,
    rows: &[StoredAttachment],
    targets: &HashMap<String, TargetMessage>,
    attachments: &HashMap<i64, Vec<StoredAttachment>>,
) -> Option<SectionText> {
    let reaction = reaction_name(
        message.associated_message_type,
        message.associated_message_emoji.as_deref(),
    );
    let sticker = rows.iter().find(|row| row.is_sticker.unwrap_or(0) != 0);
    let plain = plain_text(message.text.as_deref(), message.attributed_body.as_deref());
    let has_text = plain.as_ref().is_some_and(|text| !text.is_empty());
    if reaction.is_none() && sticker.is_none() && !has_text && rows.is_empty() {
        return None;
    }
    if let Some(name) = reaction {
        let quote = reaction_quote(
            message.associated_message_guid.as_deref(),
            targets,
            attachments,
        );
        return Some(SectionText::Line(format!(
            "Reaction: {name} to \"{quote}\""
        )));
    }
    if let Some(sticker) = sticker {
        let name = display_filename(
            sticker.transfer_name.as_deref(),
            sticker.filename.as_deref(),
        )
        .unwrap_or_else(|| "sticker".to_string());
        return Some(SectionText::Line(format!("Sticker: {name}")));
    }
    if has_text {
        return Some(SectionText::Line(plain.unwrap_or_default()));
    }
    Some(SectionText::Blank)
}

fn reaction_quote(
    associated_guid: Option<&str>,
    targets: &HashMap<String, TargetMessage>,
    attachments: &HashMap<i64, Vec<StoredAttachment>>,
) -> String {
    let Some(raw) = associated_guid.filter(|value| !value.is_empty()) else {
        return "a message".to_string();
    };
    let guid = reaction_target_guid(raw);
    let Some(target) = targets.get(guid) else {
        return "a message".to_string();
    };
    let plain = plain_text(target.text.as_deref(), target.attributed_body.as_deref());
    let collapsed = truncate_scalars(&collapse_whitespace(plain.as_deref().unwrap_or("")), 80);
    if !collapsed.is_empty() {
        return collapsed;
    }
    attachments
        .get(&target.rowid)
        .and_then(|rows| rows.first())
        .and_then(|row| display_filename(row.transfer_name.as_deref(), row.filename.as_deref()))
        .unwrap_or_else(|| "a message".to_string())
}

fn section_body(
    t: DateTime<FixedOffset>,
    sender: &str,
    rowid: i64,
    content: Option<&str>,
) -> String {
    let stamp = t.format("%Y-%m-%d %H:%M:%S %z");
    let mut body = format!("## {stamp} \u{2014} {sender}\n\n- Id: {rowid}\n");
    if let Some(content) = content {
        body.push('\n');
        body.push_str(content);
        if !content.ends_with('\n') {
            body.push('\n');
        }
    }
    body
}

fn sender_label(message: &RawMessage, view: &ChatView, contacts: &ContactIndex) -> String {
    if message.is_from_me != 0 {
        return "Me".to_string();
    }
    let Some(handle) = message.sender_handle.as_deref() else {
        return "unknown".to_string();
    };
    if let Some(title) = contact_title(contacts, handle).filter(|title| !title.is_empty()) {
        return format!("[[{title}]]");
    }
    if view.is_direct {
        if let Some(name) = &view.direct_name {
            return name.clone();
        }
    }
    handle.to_string()
}

fn collected_attachments(rows: &[StoredAttachment]) -> Vec<Attachment> {
    rows.iter()
        .map(|row| Attachment {
            filename: display_filename(row.transfer_name.as_deref(), row.filename.as_deref()),
            media_type: match row.mime_type.as_deref() {
                Some(mime) if !mime.is_empty() => Some(mime.to_string()),
                _ => None,
            },
        })
        .collect()
}

fn build_chat_view(
    meta: &ChatMeta,
    members: &[String],
    self_rows: &[(Option<String>, Option<String>)],
    contacts: &ContactIndex,
) -> ChatView {
    let selves = self_handles(
        self_rows,
        meta.last_addressed_handle.as_deref(),
        meta.account_login.as_deref(),
    );
    let people = participant_list(members, &selves);
    let others: Vec<&Person> = people.iter().filter(|person| !person.is_self).collect();
    let messages_name = trimmed_display_name(meta.display_name.as_deref());
    let is_group = meta.style == 43 || others.len() > 1;
    let is_direct = !others.is_empty() && !is_group;
    let direct_name = if is_direct {
        messages_name.clone()
    } else {
        None
    };

    let (display_title, empty_title_fallback) = if others.is_empty() {
        ("Me".to_string(), "Me".to_string())
    } else if is_group {
        let joined = joined_labels(&others, contacts);
        let title = messages_name.unwrap_or_else(|| {
            if joined.is_empty() {
                "Me".to_string()
            } else {
                joined.clone()
            }
        });
        let fallback = if joined.is_empty() {
            "Me".to_string()
        } else {
            joined
        };
        (title, fallback)
    } else {
        let handle = &others[0].handle;
        let title = contact_title(contacts, handle)
            .filter(|title| !title.is_empty())
            .map(str::to_string)
            .or_else(|| direct_name.clone())
            .unwrap_or_else(|| handle.clone());
        (title, handle.clone())
    };

    let other_handles: Vec<String> = others.iter().map(|person| person.handle.clone()).collect();
    ChatView {
        stable_id: meta.guid.clone(),
        display_title,
        empty_title_fallback,
        source_url: source_url(&meta.guid, &other_handles),
        extra_front_matter: participants_yaml(&people, contacts, direct_name.as_deref()),
        direct_name,
        is_direct,
    }
}

fn self_handles(
    rows: &[(Option<String>, Option<String>)],
    last_addressed_handle: Option<&str>,
    account_login: Option<&str>,
) -> Vec<String> {
    let callers = distinct_callers(rows.iter().map(|(caller, _)| caller.as_deref()));
    if !callers.is_empty() {
        return callers;
    }
    let accounts = distinct_accounts(rows.iter().map(|(_, account)| account.as_deref()));
    if !accounts.is_empty() {
        return accounts;
    }
    if let Some(handle) = last_addressed_handle.filter(|handle| !handle.is_empty()) {
        return vec![handle.to_string()];
    }
    if let Some(handle) = account_login.filter(|handle| !handle.is_empty()) {
        return vec![handle.to_string()];
    }
    Vec::new()
}

fn distinct_callers<'a>(values: impl Iterator<Item = Option<&'a str>>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for value in values.flatten() {
        let trimmed = trim_ascii(value);
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;
        }
        out.push(trimmed.to_string());
    }
    out
}

fn distinct_accounts<'a>(values: impl Iterator<Item = Option<&'a str>>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for value in values.flatten() {
        let Some(handle) = account_handle(value) else {
            continue;
        };
        if seen.insert(handle.clone()) {
            out.push(handle);
        }
    }
    out
}

fn account_handle(account: &str) -> Option<String> {
    let stripped = account
        .strip_prefix("E:")
        .or_else(|| account.strip_prefix("P:"))
        .unwrap_or(account);
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}

fn trim_ascii(value: &str) -> &str {
    value.trim_matches(|ch: char| ch.is_ascii_whitespace())
}

fn participant_list(members: &[String], selves: &[String]) -> Vec<Person> {
    let self_set: HashSet<&str> = selves.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    let mut people = Vec::new();
    for handle in members.iter().chain(selves.iter()) {
        if !seen.insert(handle.clone()) {
            continue;
        }
        people.push(Person {
            is_self: self_set.contains(handle.as_str()),
            handle: handle.clone(),
        });
    }
    people.sort_by(|left, right| left.handle.as_bytes().cmp(right.handle.as_bytes()));
    people
}

fn trimmed_display_name(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn contact_title<'a>(contacts: &'a ContactIndex, handle: &str) -> Option<&'a str> {
    if handle.contains('@') {
        contacts.title_for_email(handle)
    } else {
        contacts.title_for_phone(handle)
    }
}

fn joined_labels(others: &[&Person], contacts: &ContactIndex) -> String {
    let mut labels: Vec<(String, &str)> = others
        .iter()
        .map(|person| {
            let label = contact_title(contacts, &person.handle)
                .filter(|title| !title.is_empty())
                .unwrap_or(person.handle.as_str())
                .to_string();
            (label, person.handle.as_str())
        })
        .collect();
    labels.sort_by(|left, right| {
        left.0
            .as_bytes()
            .cmp(right.0.as_bytes())
            .then(left.1.as_bytes().cmp(right.1.as_bytes()))
    });
    labels
        .into_iter()
        .map(|(label, _)| label)
        .collect::<Vec<_>>()
        .join(", ")
}

fn participants_yaml(
    people: &[Person],
    contacts: &ContactIndex,
    direct_name: Option<&str>,
) -> String {
    if people.is_empty() {
        return "participants: []\n".to_string();
    }
    let mut yaml = String::from("participants:\n");
    for person in people {
        if let Some(name) = participant_name(contacts, person, direct_name) {
            yaml.push_str("  - name: ");
            yaml.push_str(&yaml_double_quoted(&name));
            yaml.push('\n');
            yaml.push_str("    handle: ");
        } else {
            yaml.push_str("  - handle: ");
        }
        yaml.push_str(&yaml_double_quoted(&person.handle));
        yaml.push('\n');
        if person.is_self {
            yaml.push_str("    self: true\n");
        }
    }
    yaml
}

fn participant_name(
    contacts: &ContactIndex,
    person: &Person,
    direct_name: Option<&str>,
) -> Option<String> {
    if let Some(title) = contact_title(contacts, &person.handle).filter(|title| !title.is_empty()) {
        return Some(title.to_string());
    }
    if !person.is_self {
        if let Some(name) = direct_name.filter(|name| !name.is_empty()) {
            return Some(name.to_string());
        }
    }
    None
}

fn source_url(guid: &str, other_handles: &[String]) -> String {
    if other_handles.is_empty() {
        return "messages://".to_string();
    }
    let scheme = if guid.as_bytes().starts_with(b"SMS;") {
        "sms"
    } else {
        "imessage"
    };
    let joined = other_handles
        .iter()
        .map(|handle| percent_encode_handle(handle))
        .collect::<Vec<_>>()
        .join(",");
    format!("{scheme}://{joined}")
}

fn percent_encode_handle(handle: &str) -> String {
    let mut encoded = String::new();
    for &byte in handle.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn unique_ids(ids: impl Iterator<Item = i64>) -> Vec<i64> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for id in ids {
        if seen.insert(id) {
            out.push(id);
        }
    }
    out
}

fn query_map_in<T>(
    conn: &Connection,
    prefix: &str,
    suffix: &str,
    ids: &[i64],
    mut map: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>, SourceError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let marks = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("{prefix}{marks}{suffix}");
    let mut stmt = conn.prepare(&sql).map_err(read_failure)?;
    let mut rows = stmt
        .query(rusqlite::params_from_iter(ids))
        .map_err(read_failure)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(read_failure)? {
        out.push(map(row).map_err(read_failure)?);
    }
    Ok(out)
}

fn load_attachments(
    conn: &Connection,
    message_ids: &[i64],
) -> Result<HashMap<i64, Vec<StoredAttachment>>, SourceError> {
    let rows = query_map_in(
        conn,
        "SELECT maj.message_id, a.ROWID, a.filename, a.mime_type, a.transfer_name, a.is_sticker
         FROM message_attachment_join AS maj
         JOIN attachment AS a ON a.ROWID = maj.attachment_id
         WHERE maj.message_id IN (",
        ") ORDER BY a.ROWID",
        message_ids,
        |row| {
            Ok(StoredAttachment {
                message_id: row.get(0)?,
                rowid: row.get(1)?,
                filename: row.get(2)?,
                mime_type: row.get(3)?,
                transfer_name: row.get(4)?,
                is_sticker: row.get(5)?,
            })
        },
    )?;
    let mut grouped: HashMap<i64, Vec<StoredAttachment>> = HashMap::new();
    for row in rows {
        grouped.entry(row.message_id).or_default().push(row);
    }
    for list in grouped.values_mut() {
        list.sort_by_key(|row| row.rowid);
    }
    Ok(grouped)
}

fn load_targets(
    conn: &Connection,
    guids: &[String],
) -> Result<HashMap<String, TargetMessage>, SourceError> {
    if guids.is_empty() {
        return Ok(HashMap::new());
    }
    let marks = std::iter::repeat_n("?", guids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql =
        format!("SELECT ROWID, guid, text, attributedBody FROM message WHERE guid IN ({marks})");
    let mut stmt = conn.prepare(&sql).map_err(read_failure)?;
    let mut rows = stmt
        .query(rusqlite::params_from_iter(guids))
        .map_err(read_failure)?;
    let mut targets: HashMap<String, TargetMessage> = HashMap::new();
    while let Some(row) = rows.next().map_err(read_failure)? {
        let rowid: i64 = row.get(0).map_err(read_failure)?;
        let guid: Option<String> = row.get(1).map_err(read_failure)?;
        let Some(guid) = guid else {
            continue;
        };
        let candidate = TargetMessage {
            rowid,
            text: row.get(2).map_err(read_failure)?,
            attributed_body: row.get(3).map_err(read_failure)?,
        };
        match targets.get(&guid) {
            Some(existing) if existing.rowid <= candidate.rowid => {}
            _ => {
                targets.insert(guid, candidate);
            }
        }
    }
    Ok(targets)
}

fn merge_target_attachments(
    conn: &Connection,
    targets: &HashMap<String, TargetMessage>,
    attachments: &mut HashMap<i64, Vec<StoredAttachment>>,
) -> Result<(), SourceError> {
    let missing: Vec<i64> = targets
        .values()
        .map(|target| target.rowid)
        .filter(|rowid| !attachments.contains_key(rowid))
        .collect();
    let extra = load_attachments(conn, &unique_ids(missing.into_iter()))?;
    for (id, rows) in extra {
        attachments.entry(id).or_default().extend(rows);
    }
    for rows in attachments.values_mut() {
        rows.sort_by_key(|row| row.rowid);
    }
    Ok(())
}

fn load_members(
    conn: &Connection,
    chat_ids: &[i64],
) -> Result<HashMap<i64, Vec<String>>, SourceError> {
    let rows = query_map_in(
        conn,
        "SELECT chj.chat_id, h.id
         FROM chat_handle_join AS chj
         JOIN handle AS h ON h.ROWID = chj.handle_id
         WHERE chj.chat_id IN (",
        ")",
        chat_ids,
        |row| {
            let chat_id: i64 = row.get(0)?;
            let handle: Option<String> = row.get(1)?;
            Ok((chat_id, handle))
        },
    )?;
    let mut grouped: HashMap<i64, Vec<String>> = HashMap::new();
    for (chat_id, handle) in rows {
        if let Some(handle) = handle {
            grouped.entry(chat_id).or_default().push(handle);
        }
    }
    Ok(grouped)
}

fn load_self_rows(
    conn: &Connection,
    chat_ids: &[i64],
) -> Result<HashMap<i64, Vec<(Option<String>, Option<String>)>>, SourceError> {
    let rows = query_map_in(
        conn,
        "SELECT cmj.chat_id, m.destination_caller_id, m.account
         FROM message AS m
         JOIN chat_message_join AS cmj ON cmj.message_id = m.ROWID
         WHERE cmj.chat_id IN (",
        ")",
        chat_ids,
        |row| {
            let chat_id: i64 = row.get(0)?;
            let caller: Option<String> = row.get(1)?;
            let account: Option<String> = row.get(2)?;
            Ok((chat_id, caller, account))
        },
    )?;
    let mut grouped: HashMap<i64, Vec<(Option<String>, Option<String>)>> = HashMap::new();
    for (chat_id, caller, account) in rows {
        grouped.entry(chat_id).or_default().push((caller, account));
    }
    Ok(grouped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use rusqlite::Connection;
    use std::fs;
    use std::path::Path;

    fn la() -> Tz {
        chrono_tz::America::Los_Angeles
    }

    fn day_slice(year: i32, month: u32, day: u32) -> Slice {
        crate::timeutil::whole_day_slice(NaiveDate::from_ymd_opt(year, month, day).unwrap(), la())
    }

    fn open_fixture() -> Connection {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/messages");
        let schema = fs::read_to_string(root.join("schema.sql")).unwrap();
        let seed = fs::read_to_string(root.join("seed.sql")).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&schema).unwrap();
        conn.execute_batch(&seed).unwrap();
        conn
    }

    fn ada_contacts() -> ContactIndex {
        let vault = crate::testutil::TempVault::new();
        let dir = vault.root.join("contacts");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("Ada Lovelace.md"),
            "---\nid: \"ada\"\nphones:\n  - value: \"+15551212\"\n---\n",
        )
        .unwrap();
        ContactIndex::load(&dir)
    }

    fn fetch_on(year: i32, month: u32, day: u32) -> Vec<CollectedItem> {
        let conn = open_fixture();
        let contacts = ada_contacts();
        fetch_messages(&conn, &day_slice(year, month, day), &contacts, &la()).unwrap()
    }

    fn message_id(item: &CollectedItem) -> i64 {
        item.body
            .lines()
            .find_map(|line| line.strip_prefix("- Id: "))
            .unwrap()
            .parse()
            .unwrap()
    }

    fn item_with(items: &[CollectedItem], id: i64) -> &CollectedItem {
        items
            .iter()
            .find(|item| message_id(item) == id)
            .unwrap_or_else(|| panic!("missing message {id}"))
    }

    fn chat<'a>(items: &'a [CollectedItem], stable_id: &str) -> &'a CollectedItem {
        items
            .iter()
            .find(|item| item.stable_id == stable_id)
            .unwrap_or_else(|| panic!("missing chat {stable_id}"))
    }

    fn sqlite_failure(code: ErrorCode, message: &str) -> Error {
        Error::SqliteFailure(
            rusqlite::ffi::Error {
                code,
                extended_code: 0,
            },
            Some(message.to_string()),
        )
    }

    #[test]
    fn apple_epoch_cutoff() {
        let seconds = apple_to_utc(812_217_660).unwrap();
        let nanos = apple_to_utc(812_217_660_000_000_000).unwrap();
        assert_eq!(seconds, nanos);
        assert_eq!(
            apple_parts(1_000_000_000_000_000),
            (1_000_000_000_000_000, 0)
        );
        assert_eq!(apple_parts(1_000_000_000_000_001), (1_000_000, 1));
    }

    #[test]
    fn uri_is_readonly() {
        let uri = sqlite_file_uri(b"/Users/ada/Library/Messages/chat.db");
        assert_eq!(uri, "file:/Users/ada/Library/Messages/chat.db?mode=ro");
        assert!(!uri.contains("immutable"));
        let spaced = sqlite_file_uri(b"/Users/ada/My Messages/chat.db");
        assert_eq!(spaced, "file:/Users/ada/My%20Messages/chat.db?mode=ro");
        assert!(!spaced.contains("immutable"));

        let vault = crate::testutil::TempVault::new();
        let path = vault.root.join("chat.db");
        let setup = Connection::open(&path).unwrap();
        setup
            .execute_batch("CREATE TABLE t(x); INSERT INTO t VALUES (1);")
            .unwrap();
        drop(setup);
        let bytes = std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str());
        let flags =
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI;
        let conn = Connection::open_with_flags(sqlite_file_uri(bytes), flags).unwrap();
        conn.execute_batch("PRAGMA query_only = ON; PRAGMA busy_timeout = 0;")
            .unwrap();
        let value: i64 = conn
            .query_row("SELECT x FROM t", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, 1);
        assert!(conn.execute("CREATE TABLE u(x)", []).is_err());
    }

    #[test]
    fn open_error_reasons() {
        let locked = "Messages database is locked";
        let unreadable = "Messages database is not readable (Full Disk Access required)";
        let busy = open_failure_reason(
            &sqlite_failure(ErrorCode::DatabaseBusy, "See you there."),
            false,
        );
        let locked_reason = open_failure_reason(
            &sqlite_failure(ErrorCode::DatabaseLocked, "See you there."),
            false,
        );
        let permitted = open_failure_reason(
            &sqlite_failure(ErrorCode::CannotOpen, "Operation not permitted"),
            false,
        );
        let denied = open_failure_reason(
            &sqlite_failure(ErrorCode::CannotOpen, "PERMISSION DENIED"),
            false,
        );
        let errno = open_failure_reason(
            &sqlite_failure(ErrorCode::CannotOpen, "unable to open database file"),
            true,
        );
        let missing = open_failure_reason(
            &sqlite_failure(ErrorCode::CannotOpen, "unable to open database file"),
            false,
        );
        assert_eq!(busy, locked);
        assert_eq!(locked_reason, locked);
        assert_eq!(permitted, unreadable);
        assert_eq!(denied, unreadable);
        assert_eq!(errno, unreadable);
        assert_eq!(
            missing,
            "cannot open Messages database: unable to open database file"
        );
        for reason in [&busy, &locked_reason, &permitted, &denied, &errno, &missing] {
            assert!(!reason.contains("See you there."));
        }
    }

    #[test]
    fn slice_uses_sent_time() {
        let items = fetch_on(2026, 9, 27);
        let row = item_with(&items, 100);
        assert_eq!(
            row.body,
            "## 2026-09-27 09:01:00 -0700 \u{2014} [[Ada Lovelace]]\n\n- Id: 100\n\nSee you there.\n"
        );
        assert!(items.iter().all(|item| message_id(item) != 110));
        assert!(items.iter().all(|item| !item.body.contains("too early")));
    }

    #[test]
    fn file_day_follows_sent_instant() {
        let slice_a = fetch_on(2026, 9, 27);
        assert_eq!(
            item_with(&slice_a, 111).file_day,
            NaiveDate::from_ymd_opt(2026, 9, 27).unwrap()
        );
        assert!(slice_a.iter().all(|item| message_id(item) != 700));

        let slice_b = fetch_on(2026, 9, 28);
        let row = item_with(&slice_b, 700);
        assert_eq!(row.file_day, NaiveDate::from_ymd_opt(2026, 9, 28).unwrap());
        assert!(row.body.contains("2026-09-28 00:30:00 -0700"));
        assert_eq!(
            slice_b.iter().map(message_id).collect::<Vec<_>>(),
            vec![700]
        );
    }

    #[test]
    fn nanosecond_heading_matches_seconds() {
        let items = fetch_on(2026, 9, 27);
        let body = &item_with(&items, 101).body;
        assert!(body.starts_with("## 2026-09-27 09:02:00 -0700 \u{2014} Me\n"));
        assert!(body.contains("On my way\n"));
    }

    #[test]
    fn direct_group_sms_and_self_titles() {
        let items = fetch_on(2026, 9, 27);
        let expected = [
            (
                "iMessage;-;+15551212",
                "Ada Lovelace",
                "+15551212",
                "imessage://%2B15551212",
            ),
            (
                "iMessage;+;chat555",
                "Ada Lovelace, ada@icloud.com",
                "Ada Lovelace, ada@icloud.com",
                "imessage://%2B15551212,ada%40icloud.com",
            ),
            (
                "SMS;-;+15559876",
                "+15559876",
                "+15559876",
                "sms://%2B15559876",
            ),
            ("iMessage;-;self", "Me", "Me", "messages://"),
            (
                "iMessage;-;+15550000",
                "Sam",
                "+15550000",
                "imessage://%2B15550000",
            ),
            (
                "iMessage;+;chat777",
                "Book club",
                "Ada Lovelace, ada@icloud.com",
                "imessage://%2B15551212,ada%40icloud.com",
            ),
        ];
        for (stable_id, title, fallback, url) in expected {
            let item = chat(&items, stable_id);
            assert_eq!(item.display_title, title, "{stable_id}");
            assert_eq!(item.empty_title_fallback, fallback, "{stable_id}");
            assert_eq!(item.source_url, url, "{stable_id}");
        }
    }

    #[test]
    fn group_url_includes_silent_participant() {
        let items = fetch_on(2026, 9, 27);
        let url = &chat(&items, "iMessage;+;chat555").source_url;
        assert!(url.contains("ada%40icloud.com"));
        let chat2: Vec<i64> = items
            .iter()
            .filter(|item| item.stable_id == "iMessage;+;chat555")
            .map(message_id)
            .collect();
        assert_eq!(chat2, vec![500]);
    }

    #[test]
    fn participants_yaml() {
        let items = fetch_on(2026, 9, 27);
        let chat2 = "\
participants:
  - name: \"Ada Lovelace\"
    handle: \"+15551212\"
  - handle: \"ada@icloud.com\"
  - handle: \"me@icloud.com\"
    self: true
";
        assert_eq!(chat(&items, "iMessage;+;chat555").extra_front_matter, chat2);
        assert_eq!(chat(&items, "iMessage;+;chat777").extra_front_matter, chat2);
        assert!(chat(&items, "iMessage;+;chat777")
            .extra_front_matter
            .contains("self: true"));

        let chat1 = &chat(&items, "iMessage;-;+15551212").extra_front_matter;
        assert_eq!(
            chat1,
            "\
participants:
  - name: \"Ada Lovelace\"
    handle: \"+15551212\"
  - handle: \"me@icloud.com\"
    self: true
"
        );
        assert!(!chat1.contains("other@icloud.com"));

        assert_eq!(
            chat(&items, "iMessage;-;self").extra_front_matter,
            "\
participants:
  - handle: \"me@icloud.com\"
    self: true
"
        );
        assert_eq!(
            chat(&items, "iMessage;-;+15550000").extra_front_matter,
            "\
participants:
  - name: \"Sam\"
    handle: \"+15550000\"
  - handle: \"me@icloud.com\"
    self: true
"
        );
    }

    #[test]
    fn one_item_per_message() {
        let items = fetch_on(2026, 9, 27);
        let mut ids: Vec<i64> = items.iter().map(message_id).collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![100, 101, 102, 103, 104, 105, 106, 107, 111, 112, 113, 200, 300, 400, 500, 600]
        );
        for id in [100, 102, 104, 103] {
            assert_eq!(
                items.iter().filter(|item| message_id(item) == id).count(),
                1
            );
        }
        let chat1: Vec<_> = items
            .iter()
            .filter(|item| item.stable_id == "iMessage;-;+15551212")
            .collect();
        assert!(chat1.len() > 1);
        assert!(chat1.iter().all(|item| {
            item.stable_id == chat1[0].stable_id
                && item.display_title == chat1[0].display_title
                && item.source_url == chat1[0].source_url
        }));
    }

    #[test]
    fn tie_break_is_padded_rowid() {
        let items = fetch_on(2026, 9, 27);
        assert_eq!(item_with(&items, 100).tie_break, "00000000000000000100");
        assert!("00000000000000000100" < "00000000000000000101");
    }

    #[test]
    fn reaction_quote_and_names() {
        let items = fetch_on(2026, 9, 27);
        assert!(item_with(&items, 102)
            .body
            .contains("Reaction: Like to \"dinner at 7\"\n"));
        assert!(item_with(&items, 106)
            .body
            .contains("Reaction: Love to \"pic.jpg\"\n"));
        assert!(item_with(&items, 107)
            .body
            .contains("Reaction: Laugh to \"a message\"\n"));
        let mut quote = "a".repeat(79);
        quote.push('\u{00e9}');
        let emphasize = item_with(&items, 112);
        assert!(emphasize
            .body
            .contains(&format!("Reaction: Emphasize to \"{quote}\"\n")));
        assert!(!emphasize.body.contains('Z'));
        assert!(!emphasize.body.contains('…'));
        assert!(!emphasize.body.contains("..."));
        assert!(item_with(&items, 113)
            .body
            .contains("Reaction: Cheer to \"See you there.\"\n"));
        assert_eq!(reaction_name(Some(2002), None).as_deref(), Some("Dislike"));
        assert_eq!(reaction_name(Some(2005), None).as_deref(), Some("Question"));
        assert_eq!(
            reaction_name(Some(2001), Some("Cheer")).as_deref(),
            Some("Like")
        );
    }

    #[test]
    fn sticker_and_attachment_metadata() {
        let items = fetch_on(2026, 9, 27);
        let sticker = item_with(&items, 104);
        assert!(sticker.body.contains("Sticker: heart.png\n"));
        assert_eq!(sticker.attachments.len(), 1);
        assert_eq!(
            sticker.attachments[0].filename.as_deref(),
            Some("heart.png")
        );
        assert_eq!(
            sticker.attachments[0].media_type.as_deref(),
            Some("image/png")
        );

        let unnamed = item_with(&items, 105);
        assert!(unnamed.body.contains("Sticker: sticker\n"));
        assert_eq!(unnamed.attachments.len(), 1);
        assert!(unnamed.attachments[0].filename.is_none());
        assert!(unnamed.attachments[0].media_type.is_none());

        let photo = item_with(&items, 103);
        assert!(photo.body.ends_with("- Id: 103\n"));
        assert_eq!(photo.attachments.len(), 1);
        assert_eq!(photo.attachments[0].filename.as_deref(), Some("pic.jpg"));
        assert_eq!(
            photo.attachments[0].media_type.as_deref(),
            Some("image/jpeg")
        );
        assert!(!photo.body.contains("pic-on-disk.jpg"));
        assert!(photo
            .attachments
            .iter()
            .all(|attachment| attachment.filename.as_deref() != Some("pic-on-disk.jpg")));
    }

    #[test]
    fn text_column_wins() {
        let items = fetch_on(2026, 9, 27);
        let body = &item_with(&items, 100).body;
        assert!(body.contains("See you there.\n"));
        assert!(!body.contains("not-a-stream"));
    }

    #[test]
    fn omit_unsent_and_retracted() {
        let conn = open_fixture();
        let contacts = ada_contacts();
        let fetched = fetch_messages(&conn, &day_slice(2026, 9, 27), &contacts, &la());
        let items = fetched.expect("fetch error");
        assert!(items.iter().all(|item| message_id(item) != 108));
        assert!(items.iter().all(|item| message_id(item) != 109));
        for item in &items {
            assert!(!item.body.contains("secret unsent body"));
            assert!(!item.body.contains("secret retracted body"));
        }
    }

    #[test]
    fn source_url_encoder() {
        let handles = vec!["+15551212".to_string(), "ada@icloud.com".to_string()];
        assert_eq!(
            source_url("SMS;+;chat1", &handles),
            "sms://%2B15551212,ada%40icloud.com"
        );
        assert_eq!(source_url("iMessage;-;self", &[]), "messages://");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn messages_source_requires_macos() {
        let source = MessagesSource {
            zone: chrono_tz::UTC,
        };
        let slice = crate::timeutil::whole_day_slice(
            NaiveDate::from_ymd_opt(2026, 9, 27).unwrap(),
            chrono_tz::UTC,
        );
        let err = source.fetch(&slice, &ContactIndex::empty()).unwrap_err();
        assert_eq!(err.reason, "Messages is only available on macOS");
    }
}
