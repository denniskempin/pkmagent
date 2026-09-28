//! Gmail collection and installed-app authorization.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use chrono_tz::Tz;
use rand::RngCore;
use scraper::{ElementRef, Html, Node};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::contacts_index::ContactIndex;
use crate::sources::{Attachment, CollectSource, CollectedItem, SourceError};
use crate::timeutil::Slice;

const AUTH_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const GMAIL_SCOPE: &str = "https://www.googleapis.com/auth/gmail.readonly";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const PROFILE_URL: &str = "https://gmail.googleapis.com/gmail/v1/users/me/profile";
const MESSAGES_URL: &str = "https://gmail.googleapis.com/gmail/v1/users/me/messages";
const AUTH_OK_BODY: &str = "pkmagent authorized this Gmail account. You can close this window.";

#[derive(Debug)]
pub struct GmailAuthError {
    pub reason: String,
}

impl std::fmt::Display for GmailAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for GmailAuthError {}

pub struct GmailSource {
    pub secrets_dir: PathBuf,
    pub zone: Tz,
}

impl CollectSource for GmailSource {
    fn fetch(
        &self,
        slice: &Slice,
        contacts: &ContactIndex,
    ) -> Result<Vec<CollectedItem>, SourceError> {
        fetch_with(self, slice, contacts, &ProductionHttp)
    }

    fn group_extra_front_matter(&self, items: &[CollectedItem]) -> String {
        group_front_matter(items)
    }
}

pub fn authorize(secrets_dir: &Path) -> Result<(), GmailAuthError> {
    authorize_with(secrets_dir, &ProductionHttp, &SystemOpener)
}

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

trait GmailHttp {
    fn execute(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, ()>;
}

trait UrlOpener {
    fn open(&self, url: &str) -> bool;
}

struct ProductionHttp;

impl GmailHttp for ProductionHttp {
    fn execute(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, ()> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .use_rustls_tls()
            .build()
            .map_err(|_| ())?;
        let mut request = match method {
            "POST" => client.post(url),
            _ => client.get(url),
        };
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        if let Some(body) = body {
            request = request.body(body.to_vec());
        }
        let response = request.send().map_err(|_| ())?;
        let status = response.status().as_u16();
        let body = response.bytes().map_err(|_| ())?.to_vec();
        Ok(HttpResponse { status, body })
    }
}

struct SystemOpener;

impl UrlOpener for SystemOpener {
    fn open(&self, url: &str) -> bool {
        open_browser(url)
    }
}

fn open_browser(url: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = url;
        false
    }
}

fn auth_err(reason: &str) -> GmailAuthError {
    GmailAuthError {
        reason: reason.to_string(),
    }
}

fn source_err(reason: impl Into<String>) -> SourceError {
    SourceError::new(reason)
}

fn authorize_with(
    secrets_dir: &Path,
    http: &dyn GmailHttp,
    opener: &dyn UrlOpener,
) -> Result<(), GmailAuthError> {
    let client = read_client(secrets_dir).map_err(auth_err)?;
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|_| auth_err("Gmail authorization failed"))?;
    listener
        .set_nonblocking(true)
        .map_err(|_| auth_err("Gmail authorization failed"))?;
    let port = listener
        .local_addr()
        .map_err(|_| auth_err("Gmail authorization failed"))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/");
    let verifier = pkce_verifier();
    let challenge = pkce_challenge(&verifier);
    let state = random_hex(16);
    let url = authorization_url(&client.client_id, &redirect_uri, &challenge, &state);
    if !opener.open(&url) {
        eprintln!("{url}");
    }
    let callback = accept_callback(&listener)?;
    if callback.state.as_deref() != Some(state.as_str()) {
        return Err(auth_err("Gmail authorization state did not match"));
    }
    if let Some(error) = callback.error.as_deref() {
        if error == "access_denied" {
            return Err(auth_err("Gmail authorization was denied"));
        }
        return Err(auth_err("Gmail authorization failed"));
    }
    let Some(code) = callback.code.filter(|code| !code.is_empty()) else {
        return Err(auth_err("Gmail authorization failed"));
    };
    let form = form_encode(&[
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("client_id", &client.client_id),
        ("client_secret", &client.client_secret),
        ("redirect_uri", &redirect_uri),
        ("code_verifier", &verifier),
    ]);
    let response = http
        .execute(
            "POST",
            TOKEN_URL,
            &[("Content-Type", "application/x-www-form-urlencoded")],
            Some(form.as_bytes()),
        )
        .map_err(|_| auth_err("Gmail authorization failed"))?;
    let refresh = refresh_token_from_auth(&response)?;
    write_refresh_token(secrets_dir, &refresh).map_err(auth_err)?;
    Ok(())
}

struct CallbackQuery {
    code: Option<String>,
    error: Option<String>,
    state: Option<String>,
}

fn accept_callback(listener: &TcpListener) -> Result<CallbackQuery, GmailAuthError> {
    let deadline = Instant::now() + AUTH_TIMEOUT;
    loop {
        if Instant::now() >= deadline {
            return Err(auth_err("Gmail authorization timed out"));
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
                let Ok((path, query)) = read_request_target(&mut stream) else {
                    continue;
                };
                if path == "/favicon.ico" {
                    write_http_response(
                        &mut stream,
                        404,
                        "Not Found",
                        "text/plain; charset=utf-8",
                        "",
                    );
                    continue;
                }
                write_http_response(
                    &mut stream,
                    200,
                    "OK",
                    "text/html; charset=utf-8",
                    AUTH_OK_BODY,
                );
                let params = query_pairs(&query);
                return Ok(CallbackQuery {
                    code: params.get("code").cloned(),
                    error: params.get("error").cloned(),
                    state: params.get("state").cloned(),
                });
            }
            Err(err)
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::Interrupted =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return Err(auth_err("Gmail authorization failed")),
        }
    }
}

fn read_request_target(stream: &mut TcpStream) -> Result<(String, String), ()> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    while !buf.windows(4).any(|window| window == b"\r\n\r\n") {
        if buf.len() > 65_536 {
            return Err(());
        }
        let read = stream.read(&mut tmp).map_err(|_| ())?;
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..read]);
    }
    let text = String::from_utf8_lossy(&buf);
    let line = text.lines().next().unwrap_or("");
    let mut parts = line.split_whitespace();
    let _method = parts.next();
    let target = parts.next().unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Ok((path.to_string(), query.to_string()))
}

fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &str,
) {
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

fn query_pairs(query: &str) -> std::collections::HashMap<String, String> {
    let mut pairs = std::collections::HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = urlencoding::decode(key).unwrap_or(std::borrow::Cow::Borrowed(key));
        let value = urlencoding::decode(value).unwrap_or(std::borrow::Cow::Borrowed(value));
        pairs.insert(key.into_owned(), value.into_owned());
    }
    pairs
}

fn refresh_token_from_auth(response: &HttpResponse) -> Result<String, GmailAuthError> {
    let Ok(value) = serde_json::from_slice::<Value>(&response.body) else {
        return Err(auth_err("Gmail authorization failed"));
    };
    let token = value
        .get("refresh_token")
        .and_then(|token| token.as_str())
        .filter(|token| !token.is_empty())
        .map(str::to_string);
    if (200..300).contains(&response.status) {
        return token.ok_or_else(|| auth_err("Gmail did not return a refresh token"));
    }
    if token.is_none() {
        return Err(auth_err("Gmail did not return a refresh token"));
    }
    Err(auth_err("Gmail authorization failed"))
}

fn fetch_with(
    source: &GmailSource,
    slice: &Slice,
    contacts: &ContactIndex,
    http: &dyn GmailHttp,
) -> Result<Vec<CollectedItem>, SourceError> {
    let client = read_client(&source.secrets_dir).map_err(source_err)?;
    let refresh_token = read_refresh_token(&source.secrets_dir).map_err(source_err)?;
    let access_token = refresh_access_token(http, &source.secrets_dir, &client, &refresh_token)?;
    let account = read_profile(http, &access_token)?;
    let ids = list_message_ids(http, &access_token, slice)?;
    let mut items = Vec::new();
    for id in ids {
        let message = get_message(http, &access_token, &id)?;
        let item = map_message(
            &message,
            &account,
            source.zone,
            contacts,
            http,
            &access_token,
        )?;
        if in_slice(item.t, slice) {
            items.push(item);
        }
    }
    Ok(items)
}

fn in_slice(t: DateTime<FixedOffset>, slice: &Slice) -> bool {
    let millis = t.timestamp_millis();
    millis >= slice.start.timestamp_millis() && millis < slice.end.timestamp_millis()
}

struct ClientCreds {
    client_id: String,
    client_secret: String,
}

fn read_client(secrets_dir: &Path) -> Result<ClientCreds, &'static str> {
    let text = fs::read_to_string(secrets_dir.join("gmail-client.json"))
        .map_err(|_| "gmail-client.json is invalid")?;
    let value: Value = serde_json::from_str(&text).map_err(|_| "gmail-client.json is invalid")?;
    let Some(object) = value.as_object() else {
        return Err("gmail-client.json is invalid");
    };
    let client_id = object
        .get("client_id")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let client_secret = object
        .get("client_secret")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if client_id.is_empty() || client_secret.is_empty() {
        return Err("gmail-client.json is invalid");
    }
    Ok(ClientCreds {
        client_id: client_id.to_string(),
        client_secret: client_secret.to_string(),
    })
}

fn read_refresh_token(secrets_dir: &Path) -> Result<String, &'static str> {
    let path = secrets_dir.join("gmail.json");
    if !path.exists() {
        return Err("Gmail is not authorized");
    }
    let text = fs::read_to_string(&path).map_err(|_| "gmail.json is invalid")?;
    let value: Value = serde_json::from_str(&text).map_err(|_| "gmail.json is invalid")?;
    let token = value
        .get("refresh_token")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if token.is_empty() {
        return Err("gmail.json is invalid");
    }
    Ok(token.to_string())
}

fn write_refresh_token(secrets_dir: &Path, refresh_token: &str) -> Result<(), &'static str> {
    let dest = secrets_dir.join("gmail.json");
    let tmp = secrets_dir.join(format!(".{}.tmp", random_hex(8)));
    let write_result = (|| -> Result<(), &'static str> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|_| "writing gmail.json failed")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|_| "writing gmail.json failed")?;
        }
        let mut text = serde_json::to_string(&serde_json::json!({
            "refresh_token": refresh_token
        }))
        .map_err(|_| "writing gmail.json failed")?;
        text.push('\n');
        file.write_all(text.as_bytes())
            .map_err(|_| "writing gmail.json failed")?;
        file.sync_all().map_err(|_| "writing gmail.json failed")?;
        Ok(())
    })();
    if let Err(err) = write_result {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    if fs::rename(&tmp, &dest).is_err() {
        let _ = fs::remove_file(&tmp);
        return Err("writing gmail.json failed");
    }
    Ok(())
}

fn refresh_access_token(
    http: &dyn GmailHttp,
    secrets_dir: &Path,
    client: &ClientCreds,
    refresh_token: &str,
) -> Result<String, SourceError> {
    let form = form_encode(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", &client.client_id),
        ("client_secret", &client.client_secret),
    ]);
    let response = http
        .execute(
            "POST",
            TOKEN_URL,
            &[("Content-Type", "application/x-www-form-urlencoded")],
            Some(form.as_bytes()),
        )
        .map_err(|_| source_err("refreshing the Gmail credential failed"))?;
    if !(200..300).contains(&response.status) {
        return Err(source_err("refreshing the Gmail credential failed"));
    }
    let value: Value = serde_json::from_slice(&response.body)
        .map_err(|_| source_err("refreshing the Gmail credential failed"))?;
    let access = value
        .get("access_token")
        .and_then(|token| token.as_str())
        .filter(|token| !token.is_empty())
        .ok_or_else(|| source_err("refreshing the Gmail credential failed"))?
        .to_string();
    if let Some(rotated) = value
        .get("refresh_token")
        .and_then(|token| token.as_str())
        .filter(|token| !token.is_empty())
    {
        write_refresh_token(secrets_dir, rotated).map_err(source_err)?;
    }
    Ok(access)
}

fn read_profile(http: &dyn GmailHttp, access_token: &str) -> Result<String, SourceError> {
    let response = api_get(http, PROFILE_URL, access_token)
        .map_err(|_| source_err("reading the Gmail profile failed"))?;
    if !(200..300).contains(&response.status) {
        return Err(source_err("reading the Gmail profile failed"));
    }
    let value: Value = serde_json::from_slice(&response.body)
        .map_err(|_| source_err("reading the Gmail profile failed"))?;
    value
        .get("emailAddress")
        .and_then(|email| email.as_str())
        .filter(|email| !email.is_empty())
        .map(str::to_string)
        .ok_or_else(|| source_err("reading the Gmail profile failed"))
}

fn list_query(slice: &Slice) -> String {
    // Gmail after/before are second-granularity. Widen by one second, then filter on internalDate.
    let after = slice.start.timestamp() - 1;
    let before = slice.end.timestamp() + 1;
    format!("-in:drafts -in:spam -in:trash after:{after} before:{before}")
}

fn list_messages_url(query: &str, page_token: Option<&str>) -> String {
    let mut url = format!(
        "{MESSAGES_URL}?maxResults=500&includeSpamTrash=false&q={}",
        urlencoding::encode(query)
    );
    if let Some(token) = page_token {
        url.push_str("&pageToken=");
        url.push_str(&urlencoding::encode(token));
    }
    url
}

fn list_message_ids(
    http: &dyn GmailHttp,
    access_token: &str,
    slice: &Slice,
) -> Result<Vec<String>, SourceError> {
    let query = list_query(slice);
    let mut page_token: Option<String> = None;
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    loop {
        if let Some(token) = &page_token {
            if !seen.insert(token.clone()) {
                return Err(source_err("Gmail list pagination repeated a page token"));
            }
        }
        let url = list_messages_url(&query, page_token.as_deref());
        let response = api_get(http, &url, access_token)
            .map_err(|_| source_err("listing Gmail messages failed"))?;
        if !(200..300).contains(&response.status) {
            return Err(source_err(format!(
                "listing Gmail messages failed (HTTP {})",
                response.status
            )));
        }
        let value: Value = serde_json::from_slice(&response.body).map_err(|_| {
            source_err(format!(
                "listing Gmail messages failed (HTTP {})",
                response.status
            ))
        })?;
        if let Some(messages) = value
            .get("messages")
            .and_then(|messages| messages.as_array())
        {
            for message in messages {
                if let Some(id) = message.get("id").and_then(|id| id.as_str()) {
                    if !id.is_empty() {
                        ids.push(id.to_string());
                    }
                }
            }
        }
        match value
            .get("nextPageToken")
            .and_then(|token| token.as_str())
            .filter(|token| !token.is_empty())
        {
            Some(token) => page_token = Some(token.to_string()),
            None => break,
        }
    }
    Ok(ids)
}

fn message_url(id: &str) -> String {
    format!("{MESSAGES_URL}/{}?format=full", urlencoding::encode(id))
}

fn attachment_url(message_id: &str, attachment_id: &str) -> String {
    format!(
        "{MESSAGES_URL}/{}/attachments/{}",
        urlencoding::encode(message_id),
        urlencoding::encode(attachment_id)
    )
}

fn get_message(http: &dyn GmailHttp, access_token: &str, id: &str) -> Result<Value, SourceError> {
    let response = api_get(http, &message_url(id), access_token)
        .map_err(|_| source_err("reading a Gmail message failed"))?;
    if !(200..300).contains(&response.status) {
        return Err(source_err(format!(
            "reading a Gmail message failed (HTTP {})",
            response.status
        )));
    }
    serde_json::from_slice(&response.body).map_err(|_| {
        source_err(format!(
            "reading a Gmail message failed (HTTP {})",
            response.status
        ))
    })
}

fn api_get(http: &dyn GmailHttp, url: &str, access_token: &str) -> Result<HttpResponse, ()> {
    let authorization = format!("Bearer {access_token}");
    http.execute(
        "GET",
        url,
        &[("Authorization", authorization.as_str())],
        None,
    )
}

fn map_message(
    message: &Value,
    account: &str,
    zone: Tz,
    contacts: &ContactIndex,
    http: &dyn GmailHttp,
    access_token: &str,
) -> Result<CollectedItem, SourceError> {
    let id = message
        .get("id")
        .and_then(|id| id.as_str())
        .filter(|id| !id.is_empty())
        .ok_or_else(|| source_err("reading a Gmail message failed"))?;
    let thread_id = message
        .get("threadId")
        .and_then(|id| id.as_str())
        .filter(|id| !id.is_empty())
        .ok_or_else(|| source_err("a Gmail message has no thread id"))?;
    let internal = message
        .get("internalDate")
        .and_then(|value| value.as_str())
        .ok_or_else(|| source_err("a Gmail message has no internalDate"))?;
    let millis: i64 = internal
        .parse()
        .map_err(|_| source_err("a Gmail message has no internalDate"))?;
    let utc = DateTime::<Utc>::from_timestamp_millis(millis)
        .ok_or_else(|| source_err("a Gmail message has no internalDate"))?;
    let zoned = utc.with_timezone(&zone);
    let t: DateTime<FixedOffset> = zoned.fixed_offset();
    let file_day = zoned.date_naive();
    let payload = message.get("payload").cloned().unwrap_or(Value::Null);
    let subject_raw = joined_header(&payload, "Subject");
    let subject = normalize_subject(&subject_raw);
    let display_title = if subject_raw_missing_or_blank(&payload) {
        String::new()
    } else {
        subject.clone()
    };
    let from = address_header(&payload, "From");
    let to = address_header(&payload, "To");
    let cc = address_header(&payload, "Cc");
    let bcc = address_header(&payload, "Bcc");
    let mut walked = Vec::new();
    for mailbox in from
        .iter()
        .chain(to.iter())
        .chain(cc.iter())
        .chain(bcc.iter())
    {
        walked.push(mailbox.clone());
    }
    let participants = participants_from_mailboxes(&walked, contacts);
    let extra = render_front_matter(account, &participants);
    let parts = collect_parts(&payload);
    let (markdown, cids) = message_markdown(&parts, id, http, access_token)?;
    let attachments = attachment_metadata(&parts, &cids);
    let body = render_section(
        t, id, &subject, &from, &to, &cc, &bcc, account, contacts, &markdown,
    );
    Ok(CollectedItem {
        stable_id: thread_id.to_string(),
        display_title,
        empty_title_fallback: "(no subject)".to_string(),
        source_url: source_url(account, thread_id),
        t,
        tie_break: id.to_string(),
        file_day,
        extra_front_matter: extra,
        body,
        attachments,
    })
}

fn subject_raw_missing_or_blank(payload: &Value) -> bool {
    let headers = header_values(payload, "Subject");
    if headers.is_empty() {
        return true;
    }
    normalize_subject(&headers.join(", ")).is_empty()
}

fn normalize_subject(raw: &str) -> String {
    let mut text = String::new();
    for ch in raw.chars() {
        if ch == '\r' || ch == '\n' || ch == '\t' {
            text.push(' ');
        } else {
            text.push(ch);
        }
    }
    text.trim().to_string()
}

fn source_url(account: &str, thread_id: &str) -> String {
    format!(
        "https://mail.google.com/mail/?authuser={}#all/{thread_id}",
        urlencoding::encode(account)
    )
}

#[derive(Clone)]
struct ParsedMailbox {
    display_name: Option<String>,
    email: String,
}

fn address_header(payload: &Value, name: &str) -> Vec<ParsedMailbox> {
    let joined = joined_header(payload, name);
    parse_address_list(&joined)
}

fn joined_header(payload: &Value, name: &str) -> String {
    header_values(payload, name).join(", ")
}

fn header_values(part: &Value, name: &str) -> Vec<String> {
    let Some(headers) = part.get("headers").and_then(|headers| headers.as_array()) else {
        return Vec::new();
    };
    let mut values = Vec::new();
    for header in headers {
        let header_name = header
            .get("name")
            .and_then(|name| name.as_str())
            .unwrap_or("");
        if header_name.eq_ignore_ascii_case(name) {
            if let Some(value) = header.get("value").and_then(|value| value.as_str()) {
                values.push(value.to_string());
            }
        }
    }
    values
}

fn first_header(part: &Value, name: &str) -> Option<String> {
    header_values(part, name).into_iter().next()
}

fn parse_address_list(input: &str) -> Vec<ParsedMailbox> {
    if input.trim().is_empty() {
        return Vec::new();
    }
    let Ok(list) = mailparse::addrparse(input) else {
        return Vec::new();
    };
    let mut mailboxes = Vec::new();
    for addr in list.iter() {
        match addr {
            mailparse::MailAddr::Single(info) => push_mailbox(&mut mailboxes, info),
            mailparse::MailAddr::Group(group) => {
                for info in &group.addrs {
                    push_mailbox(&mut mailboxes, info);
                }
            }
        }
    }
    mailboxes
}

fn push_mailbox(mailboxes: &mut Vec<ParsedMailbox>, info: &mailparse::SingleInfo) {
    let email = info.addr.trim();
    if email.is_empty() {
        return;
    }
    let display_name = info
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    mailboxes.push(ParsedMailbox {
        display_name,
        email: email.to_string(),
    });
}

struct Participant {
    name: Option<String>,
    email: String,
}

fn participants_from_mailboxes(
    mailboxes: &[ParsedMailbox],
    contacts: &ContactIndex,
) -> Vec<Participant> {
    let mut kept = Vec::new();
    for mailbox in mailboxes {
        let email = mailbox.email.trim();
        if email.is_empty() {
            continue;
        }
        if kept
            .iter()
            .any(|participant: &Participant| participant.email.eq_ignore_ascii_case(email))
        {
            continue;
        }
        let name = contacts
            .title_for_email(email)
            .map(str::to_string)
            .or_else(|| mailbox.display_name.clone());
        kept.push(Participant {
            name,
            email: email.to_string(),
        });
    }
    sort_participants(&mut kept);
    kept
}

fn sort_participants(participants: &mut [Participant]) {
    participants.sort_by(|left, right| {
        left.email
            .to_ascii_lowercase()
            .as_bytes()
            .cmp(right.email.to_ascii_lowercase().as_bytes())
    });
}

fn render_front_matter(account: &str, participants: &[Participant]) -> String {
    let mut text = String::new();
    text.push_str("account: ");
    text.push_str(&yaml_scalar(account));
    text.push('\n');
    if !participants.is_empty() {
        text.push_str("participants:\n");
        for participant in participants {
            match &participant.name {
                Some(name) if !name.is_empty() => {
                    text.push_str("  - name: ");
                    text.push_str(&quote_yaml(name));
                    text.push('\n');
                    text.push_str("    email: ");
                    text.push_str(&yaml_scalar(&participant.email));
                    text.push('\n');
                }
                _ => {
                    text.push_str("  - email: ");
                    text.push_str(&yaml_scalar(&participant.email));
                    text.push('\n');
                }
            }
        }
    }
    text
}

fn group_front_matter(items: &[CollectedItem]) -> String {
    let Some(first) = items.first() else {
        return String::new();
    };
    let (account, _) = parse_item_front_matter(&first.extra_front_matter);
    let mut kept = Vec::new();
    for item in items {
        let (_, participants) = parse_item_front_matter(&item.extra_front_matter);
        for participant in participants {
            let email = participant.email.trim();
            if email.is_empty() {
                continue;
            }
            if kept
                .iter()
                .any(|existing: &Participant| existing.email.eq_ignore_ascii_case(email))
            {
                continue;
            }
            kept.push(Participant {
                name: participant.name.filter(|name| !name.is_empty()),
                email: email.to_string(),
            });
        }
    }
    sort_participants(&mut kept);
    render_front_matter(&account, &kept)
}

fn parse_item_front_matter(yaml: &str) -> (String, Vec<Participant>) {
    let mut account = String::new();
    let mut participants = Vec::new();
    let mut pending_name: Option<String> = None;
    let mut saw_name = false;
    for line in yaml.split('\n') {
        let line = line.trim_end_matches('\r');
        let trimmed = line.trim();
        if let Some(rest) = line.strip_prefix("account:") {
            account = parse_yaml_scalar(rest);
            continue;
        }
        if trimmed == "participants:" {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("- name:") {
            pending_name = Some(parse_yaml_scalar(rest));
            saw_name = true;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("- email:") {
            participants.push(Participant {
                name: None,
                email: parse_yaml_scalar(rest),
            });
            pending_name = None;
            saw_name = false;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("email:") {
            let name = if saw_name { pending_name.take() } else { None };
            saw_name = false;
            participants.push(Participant {
                name,
                email: parse_yaml_scalar(rest),
            });
        }
    }
    (account, participants)
}

fn parse_yaml_scalar(raw: &str) -> String {
    let raw = raw.trim();
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        return unescape_double(&raw[1..raw.len() - 1]);
    }
    raw.to_string()
}

fn unescape_double(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('n') => out.push('\n'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn yaml_scalar(value: &str) -> String {
    if is_plain_scalar(value) {
        value.to_string()
    } else {
        quote_yaml(value)
    }
}

fn is_plain_scalar(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'.' | b'!'
                    | b'#'
                    | b'$'
                    | b'%'
                    | b'&'
                    | b'\''
                    | b'*'
                    | b'+'
                    | b'/'
                    | b'='
                    | b'?'
                    | b'^'
                    | b'_'
                    | b'`'
                    | b'{'
                    | b'|'
                    | b'}'
                    | b'~'
                    | b'@'
                    | b'-'
            )
    })
}

fn quote_yaml(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' | '\r' => out.push(' '),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn render_section(
    t: DateTime<FixedOffset>,
    id: &str,
    subject: &str,
    from: &[ParsedMailbox],
    to: &[ParsedMailbox],
    cc: &[ParsedMailbox],
    bcc: &[ParsedMailbox],
    account: &str,
    contacts: &ContactIndex,
    markdown: &str,
) -> String {
    let stamp = t.format("%Y-%m-%d %H:%M:%S %z");
    let mut text = String::new();
    match from.first() {
        Some(sender) => {
            let label = format_mailbox(sender, contacts, account, true);
            text.push_str(&format!("## {stamp} — {label}\n"));
        }
        None => text.push_str(&format!("## {stamp}\n")),
    }
    text.push('\n');
    text.push_str(&format!("- Message-Id: {id}\n"));
    let subject_line = if subject.is_empty() {
        "(no subject)"
    } else {
        subject
    };
    text.push_str(&format!("- Subject: {subject_line}\n"));
    text.push_str(&format!(
        "- To: {}\n",
        join_mailboxes(to, contacts, account)
    ));
    if !cc.is_empty() {
        text.push_str(&format!(
            "- Cc: {}\n",
            join_mailboxes(cc, contacts, account)
        ));
    }
    if !bcc.is_empty() {
        text.push_str(&format!(
            "- Bcc: {}\n",
            join_mailboxes(bcc, contacts, account)
        ));
    }
    let markdown = markdown.trim_end_matches(['\r', '\n']);
    if !markdown.is_empty() {
        text.push('\n');
        text.push_str(markdown);
        text.push('\n');
    }
    text
}

fn join_mailboxes(mailboxes: &[ParsedMailbox], contacts: &ContactIndex, account: &str) -> String {
    mailboxes
        .iter()
        .map(|mailbox| format_mailbox(mailbox, contacts, account, false))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_mailbox(
    mailbox: &ParsedMailbox,
    contacts: &ContactIndex,
    account: &str,
    heading: bool,
) -> String {
    let email = mailbox.email.trim();
    if let Some(title) = contacts.title_for_email(email) {
        return format!("[[{title}]] <{email}>");
    }
    if heading && email.eq_ignore_ascii_case(account.trim()) {
        return "Me".to_string();
    }
    match &mailbox.display_name {
        Some(name) => format!("{name} <{email}>"),
        None => email.to_string(),
    }
}

struct PartRecord {
    filename: Option<String>,
    media_type: Option<String>,
    data: Option<String>,
    attachment_id: Option<String>,
    charset: Option<String>,
    content_id: Option<String>,
    is_file: bool,
}

struct PartIndex {
    parts: Vec<PartRecord>,
    html: Option<usize>,
    plain: Option<usize>,
    files: Vec<usize>,
}

fn collect_parts(payload: &Value) -> PartIndex {
    let mut index = PartIndex {
        parts: Vec::new(),
        html: None,
        plain: None,
        files: Vec::new(),
    };
    walk_part(payload, &mut index);
    index
}

fn walk_part(part: &Value, index: &mut PartIndex) {
    if !part.is_object() {
        return;
    }
    let mime = mime_of(part);
    if mime.starts_with("multipart/") {
        if let Some(children) = part.get("parts").and_then(|parts| parts.as_array()) {
            for child in children {
                walk_part(child, index);
            }
        }
        return;
    }
    let filename = filename_of(part);
    let attachment_id = part
        .get("body")
        .and_then(|body| body.get("attachmentId"))
        .and_then(|id| id.as_str())
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    let disposition = disposition_type(part);
    let is_file = filename.is_some()
        || disposition.as_deref() == Some("attachment")
        || (attachment_id.is_some() && mime != "text/html" && mime != "text/plain");
    let record = PartRecord {
        filename,
        media_type: media_type_of(&mime),
        data: part
            .get("body")
            .and_then(|body| body.get("data"))
            .and_then(|data| data.as_str())
            .filter(|data| !data.is_empty())
            .map(str::to_string),
        attachment_id,
        charset: charset_of(part),
        content_id: content_id_of(part),
        is_file,
    };
    let at = index.parts.len();
    if !is_file && mime == "text/html" && index.html.is_none() {
        index.html = Some(at);
    }
    if !is_file && mime == "text/plain" && index.plain.is_none() {
        index.plain = Some(at);
    }
    if is_file {
        index.files.push(at);
    }
    index.parts.push(record);
    if let Some(children) = part.get("parts").and_then(|parts| parts.as_array()) {
        for child in children {
            walk_part(child, index);
        }
    }
}

fn mime_of(part: &Value) -> String {
    if let Some(mime) = part.get("mimeType").and_then(|mime| mime.as_str()) {
        let mime = mime.trim();
        if !mime.is_empty() {
            return mime
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
        }
    }
    first_header(part, "Content-Type")
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default()
}

fn media_type_of(mime: &str) -> Option<String> {
    let mime = mime.split(';').next().unwrap_or("").trim();
    if mime.is_empty() {
        None
    } else {
        Some(mime.to_ascii_lowercase())
    }
}

fn filename_of(part: &Value) -> Option<String> {
    if let Some(name) = part.get("filename").and_then(|name| name.as_str()) {
        let name = name.trim();
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    if let Some(value) = first_header(part, "Content-Disposition") {
        if let Some(name) = header_param(&value, "filename") {
            let name = name.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    if let Some(value) = first_header(part, "Content-Type") {
        if let Some(name) = header_param(&value, "name") {
            let name = name.trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn header_param(header: &str, key: &str) -> Option<String> {
    for part in header.split(';').skip(1) {
        let Some((name, value)) = part.trim().split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case(key) {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn disposition_type(part: &Value) -> Option<String> {
    let value = first_header(part, "Content-Disposition")?;
    let kind = value.split(';').next()?.trim().to_ascii_lowercase();
    if kind.is_empty() {
        None
    } else {
        Some(kind)
    }
}

fn charset_of(part: &Value) -> Option<String> {
    if let Some(value) = first_header(part, "Content-Type") {
        if let Some(charset) = header_param(&value, "charset") {
            return Some(charset);
        }
    }
    part.get("mimeType")
        .and_then(|mime| mime.as_str())
        .and_then(|mime| header_param(mime, "charset"))
}

fn content_id_of(part: &Value) -> Option<String> {
    let raw = first_header(part, "Content-ID")?;
    let normalized = normalize_content_id(&raw);
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn normalize_content_id(raw: &str) -> String {
    let trimmed = raw.trim();
    let inner = if trimmed.starts_with('<') && trimmed.ends_with('>') && trimmed.len() >= 2 {
        trimmed[1..trimmed.len() - 1].trim()
    } else {
        trimmed
    };
    inner.to_string()
}

fn message_markdown(
    parts: &PartIndex,
    message_id: &str,
    http: &dyn GmailHttp,
    access_token: &str,
) -> Result<(String, Vec<String>), SourceError> {
    if let Some(index) = parts.html {
        let text = load_part_text(&parts.parts[index], message_id, http, access_token)?;
        let (markdown, cids) = html_to_markdown(&text);
        if !markdown.trim().is_empty() {
            return Ok((markdown, cids));
        }
    }
    if let Some(index) = parts.plain {
        let text = load_part_text(&parts.parts[index], message_id, http, access_token)?;
        let plain = normalize_plain(&text);
        if !plain.trim().is_empty() {
            return Ok((plain, Vec::new()));
        }
    }
    Ok((String::new(), Vec::new()))
}

fn load_part_text(
    part: &PartRecord,
    message_id: &str,
    http: &dyn GmailHttp,
    access_token: &str,
) -> Result<String, SourceError> {
    let bytes = if let Some(data) = &part.data {
        decode_base64url(data)?
    } else if let Some(attachment_id) = &part.attachment_id {
        let response = api_get(
            http,
            &attachment_url(message_id, attachment_id),
            access_token,
        )
        .map_err(|_| source_err("reading a Gmail message failed"))?;
        if !(200..300).contains(&response.status) {
            return Err(source_err(format!(
                "reading a Gmail message failed (HTTP {})",
                response.status
            )));
        }
        let value: Value = serde_json::from_slice(&response.body)
            .map_err(|_| source_err("reading a Gmail message failed"))?;
        let data = value
            .get("data")
            .and_then(|data| data.as_str())
            .unwrap_or("");
        decode_base64url(data)?
    } else {
        Vec::new()
    };
    Ok(decode_charset(&bytes, part.charset.as_deref()))
}

fn decode_base64url(data: &str) -> Result<Vec<u8>, SourceError> {
    let cleaned: String = data
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace())
        .collect();
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }
    URL_SAFE_NO_PAD
        .decode(&cleaned)
        .or_else(|_| URL_SAFE.decode(&cleaned))
        .map_err(|_| source_err("reading a Gmail message failed"))
}

fn decode_charset(bytes: &[u8], charset: Option<&str>) -> String {
    let encoding = charset
        .and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = encoding.decode(bytes);
    newlines_to_lf(&text)
}

fn newlines_to_lf(text: &str) -> String {
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

fn normalize_plain(text: &str) -> String {
    let text = newlines_to_lf(text);
    text.trim_end_matches(|ch: char| ch.is_whitespace())
        .to_string()
}

fn attachment_metadata(parts: &PartIndex, cids: &[String]) -> Vec<Attachment> {
    let mut attachments = Vec::new();
    for index in &parts.files {
        let part = &parts.parts[*index];
        attachments.push(Attachment {
            filename: part.filename.clone(),
            media_type: part.media_type.clone(),
        });
    }
    for cid in cids {
        let matched = parts.parts.iter().enumerate().find(|(_, part)| {
            part.content_id
                .as_deref()
                .is_some_and(|content_id| content_id.eq_ignore_ascii_case(cid))
        });
        let already = match matched {
            Some((index, part)) => {
                parts.files.contains(&index)
                    || parts.html == Some(index)
                    || parts.plain == Some(index)
                    || part.is_file
            }
            None => false,
        };
        if matched.is_some() && already {
            continue;
        }
        if matched.is_some() && !already {
            let part = &parts.parts[matched.unwrap().0];
            attachments.push(Attachment {
                filename: part.filename.clone(),
                media_type: part.media_type.clone(),
            });
            continue;
        }
        attachments.push(Attachment {
            filename: None,
            media_type: None,
        });
    }
    attachments
}

struct MdCtx {
    cids: Vec<String>,
}

fn html_to_markdown(html: &str) -> (String, Vec<String>) {
    let document = Html::parse_fragment(html);
    let mut ctx = MdCtx { cids: Vec::new() };
    let blocks = render_container(document.root_element(), &mut ctx);
    let joined = join_blocks(&blocks);
    let trimmed = joined.trim();
    if trimmed.is_empty() {
        (String::new(), ctx.cids)
    } else {
        (format!("{trimmed}\n"), ctx.cids)
    }
}

macro_rules! child_is_inline {
    ($child:expr) => {
        match $child.value() {
            Node::Text(_)
            | Node::Comment(_)
            | Node::Doctype(_)
            | Node::ProcessingInstruction(_) => true,
            Node::Element(element) => matches!(
                element.name(),
                "span"
                    | "strong"
                    | "b"
                    | "em"
                    | "i"
                    | "a"
                    | "br"
                    | "code"
                    | "u"
                    | "s"
                    | "small"
                    | "font"
                    | "sub"
                    | "sup"
                    | "label"
                    | "abbr"
                    | "bdo"
                    | "cite"
                    | "kbd"
                    | "q"
                    | "samp"
                    | "var"
                    | "wbr"
                    | "mark"
                    | "del"
                    | "ins"
                    | "time"
            ),
            _ => false,
        }
    };
}

macro_rules! append_child_inline {
    ($out:ident, $child:expr, $ctx:expr) => {
        match $child.value() {
            Node::Text(text) => $out.push_str(&normalize_text(&text.text)),
            Node::Element(element) => {
                let name = element.name().to_string();
                let href = element.attr("href").map(str::to_string);
                match name.as_str() {
                    "br" => $out.push_str("  \n"),
                    "strong" | "b" => {
                        if let Some(inner) = ElementRef::wrap($child) {
                            $out.push_str(&wrap_affix("**", &render_inline_children(inner, $ctx)));
                        }
                    }
                    "em" | "i" => {
                        if let Some(inner) = ElementRef::wrap($child) {
                            $out.push_str(&wrap_affix("*", &render_inline_children(inner, $ctx)));
                        }
                    }
                    "a" => {
                        if let Some(inner) = ElementRef::wrap($child) {
                            let text = render_inline_children(inner, $ctx);
                            $out.push_str(&render_anchor(&text, href.as_deref()));
                        }
                    }
                    "script" | "style" | "noscript" | "head" | "title" => {}
                    _ => {
                        if let Some(inner) = ElementRef::wrap($child) {
                            $out.push_str(&render_inline_children(inner, $ctx));
                        }
                    }
                }
            }
            _ => {}
        }
    };
}
fn render_container(node: ElementRef<'_>, ctx: &mut MdCtx) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut inline = String::new();
    for child in node.children() {
        if child_is_inline!(child) {
            append_child_inline!(inline, child, ctx);
        } else if let Some(element) = ElementRef::wrap(child) {
            flush_inline(&mut inline, &mut blocks);
            blocks.extend(render_element(element, ctx));
        }
    }
    flush_inline(&mut inline, &mut blocks);
    blocks
}

fn flush_inline(inline: &mut String, blocks: &mut Vec<String>) {
    if inline.trim().is_empty() {
        inline.clear();
        return;
    }
    blocks.push(std::mem::take(inline));
}

fn render_element(element: ElementRef<'_>, ctx: &mut MdCtx) -> Vec<String> {
    let name = element.value().name();
    if matches!(name, "script" | "style" | "noscript" | "head" | "title") {
        return Vec::new();
    }
    if name == "blockquote" || (name == "div" && class_token(element, "gmail_quote")) {
        return render_quote(element, ctx);
    }
    if let Some(level) = name
        .strip_prefix('h')
        .and_then(|rest| rest.parse::<usize>().ok())
    {
        if (1..=6).contains(&level) {
            let text = render_inline_children(element, ctx);
            let text = text.trim();
            if text.is_empty() {
                return Vec::new();
            }
            return vec![format!("{} {text}", "#".repeat(level))];
        }
    }
    match name {
        "ul" => one_block(render_list(element, false, ctx)),
        "ol" => one_block(render_list(element, true, ctx)),
        "pre" => one_block(render_inline_children(element, ctx)),
        "table" => one_block(render_table(element, ctx)),
        "img" => render_image(element, ctx),
        _ => render_container(element, ctx),
    }
}

fn one_block(text: String) -> Vec<String> {
    if text.trim().is_empty() {
        Vec::new()
    } else {
        vec![text]
    }
}

fn render_quote(element: ElementRef<'_>, ctx: &mut MdCtx) -> Vec<String> {
    let inner = render_container(element, ctx);
    let joined = join_blocks(&inner);
    let quoted = prefix_quote(&joined);
    if quoted.trim().is_empty() {
        Vec::new()
    } else {
        vec![quoted]
    }
}

fn prefix_quote(content: &str) -> String {
    if content.trim().is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for (index, line) in content.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if line.is_empty() {
            out.push('>');
        } else {
            out.push_str("> ");
            out.push_str(line);
        }
    }
    out
}

fn render_list(element: ElementRef<'_>, ordered: bool, ctx: &mut MdCtx) -> String {
    let mut lines = Vec::new();
    let mut number = 1u32;
    for child in element.children() {
        let Some(item) = ElementRef::wrap(child) else {
            continue;
        };
        if item.value().name() != "li" {
            continue;
        }
        let marker = if ordered {
            let marker = format!("{number}. ");
            number += 1;
            marker
        } else {
            "- ".to_string()
        };
        lines.extend(render_li(item, &marker, ctx));
    }
    lines.join("\n")
}

fn render_li(item: ElementRef<'_>, marker: &str, ctx: &mut MdCtx) -> Vec<String> {
    let mut text = String::new();
    let mut extra = Vec::new();
    for child in item.children() {
        if child_is_inline!(child) {
            append_child_inline!(text, child, ctx);
            continue;
        }
        let Some(element) = ElementRef::wrap(child) else {
            continue;
        };
        let name = element.value().name();
        if name == "ul" || name == "ol" {
            let nested = render_list(element, name == "ol", ctx);
            for line in nested.lines() {
                extra.push(format!("  {line}"));
            }
        } else {
            for block in render_element(element, ctx) {
                if text.trim().is_empty() && extra.is_empty() {
                    text.push_str(block.trim());
                } else {
                    for line in block.lines() {
                        extra.push(line.to_string());
                    }
                }
            }
        }
    }
    let mut lines = vec![format!("{marker}{}", text.trim())];
    lines.extend(extra);
    lines
}

fn render_table(element: ElementRef<'_>, ctx: &mut MdCtx) -> String {
    let mut rows = Vec::new();
    collect_rows(element, ctx, &mut rows);
    rows.join("\n")
}

fn collect_rows(element: ElementRef<'_>, ctx: &mut MdCtx, rows: &mut Vec<String>) {
    for child in element.children() {
        let Some(child_el) = ElementRef::wrap(child) else {
            continue;
        };
        let name = child_el.value().name();
        if name == "tr" {
            let mut cells = Vec::new();
            for cell in child_el.children() {
                let Some(cell_el) = ElementRef::wrap(cell) else {
                    continue;
                };
                if matches!(cell_el.value().name(), "td" | "th") {
                    cells.push(render_inline_children(cell_el, ctx).trim().to_string());
                }
            }
            rows.push(cells.join(" "));
        } else if !matches!(name, "td" | "th") {
            collect_rows(child_el, ctx, rows);
        }
    }
}

fn render_image(element: ElementRef<'_>, ctx: &mut MdCtx) -> Vec<String> {
    let Some(src) = element.value().attr("src") else {
        return Vec::new();
    };
    if src.trim().is_empty() {
        return Vec::new();
    }
    if src.trim().to_ascii_lowercase().starts_with("cid:") {
        let id = cid_from_src(src);
        if !id.is_empty() {
            ctx.cids.push(id);
        }
        return Vec::new();
    }
    let alt = element.value().attr("alt").unwrap_or("");
    vec![format!("![{}]({src})", normalize_alt(alt))]
}

fn cid_from_src(src: &str) -> String {
    let trimmed = src.trim();
    let rest = trimmed.get(4..).unwrap_or("").trim();
    normalize_content_id(rest)
}

fn normalize_alt(alt: &str) -> String {
    let mut out = String::new();
    for ch in alt.chars() {
        if ch == '\n' || ch == '\r' || ch == '\u{00A0}' {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

fn class_token(element: ElementRef<'_>, token: &str) -> bool {
    element.value().attr("class").is_some_and(|class| {
        class
            .split(|ch: char| ch.is_ascii_whitespace())
            .any(|part| part == token)
    })
}

fn render_inline_children(element: ElementRef<'_>, ctx: &mut MdCtx) -> String {
    let mut out = String::new();
    for child in element.children() {
        append_child_inline!(out, child, ctx);
    }
    out
}

fn render_anchor(text: &str, href: Option<&str>) -> String {
    match href {
        Some(href) => {
            let label = if text.trim().is_empty() {
                href.to_string()
            } else {
                text.to_string()
            };
            format!("[{label}]({href})")
        }
        None => text.to_string(),
    }
}

fn wrap_affix(mark: &str, inner: &str) -> String {
    if inner.is_empty() {
        String::new()
    } else {
        format!("{mark}{inner}{mark}")
    }
}

fn normalize_text(raw: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for ch in raw.chars() {
        let ch = if ch == '\u{00A0}' { ' ' } else { ch };
        if ch == ' ' || ch == '\t' || ch == '\n' || ch == '\r' {
            space = true;
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(ch);
    }
    if space {
        out.push(' ');
    }
    out
}

fn join_blocks(blocks: &[String]) -> String {
    let mut out = String::new();
    for block in blocks {
        if block.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(block);
    }
    out
}

fn pkce_verifier() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn random_hex(nbytes: usize) -> String {
    let mut bytes = vec![0u8; nbytes];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex_encode(&bytes)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

fn authorization_url(client_id: &str, redirect_uri: &str, challenge: &str, state: &str) -> String {
    let query = form_encode(&[
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("response_type", "code"),
        ("scope", GMAIL_SCOPE),
        ("access_type", "offline"),
        ("prompt", "consent"),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
    ]);
    format!("https://accounts.google.com/o/oauth2/v2/auth?{query}")
}

fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                urlencoding::encode(key),
                urlencoding::encode(value)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempVault;
    use std::sync::{Arc, Mutex};

    fn zone() -> Tz {
        chrono_tz::America::Los_Angeles
    }

    fn midnight(year: i32, month: u32, day: u32) -> DateTime<FixedOffset> {
        zone()
            .with_ymd_and_hms(year, month, day, 0, 0, 0)
            .unwrap()
            .fixed_offset()
    }

    fn slice_between(start: DateTime<FixedOffset>, end: DateTime<FixedOffset>) -> Slice {
        Slice {
            day: start.date_naive(),
            start,
            end,
        }
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/gmail")
            .join(name)
    }

    fn read_fixture_json(name: &str) -> Value {
        let text = fs::read_to_string(fixture(name)).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    struct Recorded {
        url: String,
    }

    struct FakeHttp {
        requests: Mutex<Vec<Recorded>>,
        account: String,
        messages: Vec<Value>,
        list_status: Option<u16>,
        list_body: Option<Vec<u8>>,
        token_body: Option<Vec<u8>>,
    }

    impl FakeHttp {
        fn messages(account: &str, messages: Vec<Value>) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                account: account.to_string(),
                messages,
                list_status: None,
                list_body: None,
                token_body: None,
            }
        }

        fn urls(&self) -> Vec<String> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .map(|request| request.url.clone())
                .collect()
        }
    }

    impl GmailHttp for FakeHttp {
        fn execute(
            &self,
            _method: &str,
            url: &str,
            _headers: &[(&str, &str)],
            _body: Option<&[u8]>,
        ) -> Result<HttpResponse, ()> {
            self.requests.lock().unwrap().push(Recorded {
                url: url.to_string(),
            });
            if url.contains("oauth2.googleapis.com/token") {
                let body = self
                    .token_body
                    .clone()
                    .unwrap_or_else(|| br#"{"access_token":"access-token"}"#.to_vec());
                return Ok(HttpResponse { status: 200, body });
            }
            if url.contains("/users/me/profile") {
                let body = format!(r#"{{"emailAddress":"{}"}}"#, self.account).into_bytes();
                return Ok(HttpResponse { status: 200, body });
            }
            if url.contains("/attachments/") {
                return Ok(HttpResponse {
                    status: 200,
                    body: br#"{"data":"SGk"}"#.to_vec(),
                });
            }
            let path = url.split('?').next().unwrap_or(url);
            if path.ends_with("/messages") {
                if let Some(status) = self.list_status {
                    return Ok(HttpResponse {
                        status,
                        body: self.list_body.clone().unwrap_or_default(),
                    });
                }
                let messages: Vec<_> = self
                    .messages
                    .iter()
                    .filter_map(|message| {
                        let id = message.get("id")?.as_str()?;
                        let thread = message
                            .get("threadId")
                            .and_then(|id| id.as_str())
                            .unwrap_or("");
                        Some(serde_json::json!({"id": id, "threadId": thread}))
                    })
                    .collect();
                let body = serde_json::to_vec(&serde_json::json!({"messages": messages})).unwrap();
                return Ok(HttpResponse { status: 200, body });
            }
            if let Some(id) = path.rsplit('/').next() {
                if let Some(message) = self
                    .messages
                    .iter()
                    .find(|message| message.get("id").and_then(|value| value.as_str()) == Some(id))
                {
                    return Ok(HttpResponse {
                        status: 200,
                        body: serde_json::to_vec(message).unwrap(),
                    });
                }
            }
            Ok(HttpResponse {
                status: 404,
                body: Vec::new(),
            })
        }
    }

    struct StaticHttp {
        status: u16,
        body: Vec<u8>,
        requests: Mutex<Vec<String>>,
    }

    impl GmailHttp for StaticHttp {
        fn execute(
            &self,
            _method: &str,
            url: &str,
            _headers: &[(&str, &str)],
            _body: Option<&[u8]>,
        ) -> Result<HttpResponse, ()> {
            self.requests.lock().unwrap().push(url.to_string());
            Ok(HttpResponse {
                status: self.status,
                body: self.body.clone(),
            })
        }
    }

    fn write_secrets(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("gmail-client.json"),
            r#"{"client_id":"client-id","client_secret":"client-secret","extra":true}"#,
        )
        .unwrap();
        fs::write(
            dir.join("gmail.json"),
            "{\"refresh_token\":\"refresh-token\"}\n",
        )
        .unwrap();
    }

    fn source_in(dir: &Path) -> GmailSource {
        GmailSource {
            secrets_dir: dir.to_path_buf(),
            zone: zone(),
        }
    }

    fn map_fixture(message: &Value, account: &str, http: &dyn GmailHttp) -> CollectedItem {
        map_message(
            message,
            account,
            zone(),
            &ContactIndex::empty(),
            http,
            "access-token",
        )
        .unwrap()
    }

    fn b64(text: &str) -> String {
        URL_SAFE_NO_PAD.encode(text.as_bytes())
    }

    fn query_param(url: &str, key: &str) -> Option<String> {
        let query = url.split_once('?')?.1;
        for pair in query.split('&') {
            let (name, value) = pair.split_once('=')?;
            if name == key {
                return Some(urlencoding::decode(value).ok()?.into_owned());
            }
        }
        None
    }

    #[test]
    fn list_query_excludes_spam_trash_and_drafts() {
        let start = midnight(2026, 9, 26);
        let end = midnight(2026, 9, 27);
        assert_eq!(start.timestamp(), 1_790_406_000);
        assert_eq!(end.timestamp(), 1_790_492_400);
        let vault = TempVault::new();
        let secrets = vault.root.join("secrets");
        write_secrets(&secrets);
        let http = FakeHttp::messages("name@gmail.com", Vec::new());
        let items = fetch_with(
            &source_in(&secrets),
            &slice_between(start, end),
            &ContactIndex::empty(),
            &http,
        )
        .unwrap();
        assert!(items.is_empty());
        let list_url = http
            .urls()
            .into_iter()
            .find(|url| url.contains("/messages?"))
            .unwrap();
        assert!(list_url.contains("maxResults=500"));
        assert!(list_url.contains("includeSpamTrash=false"));
        assert!(!list_url.contains("pageToken"));
        let query = query_param(&list_url, "q").unwrap();
        assert_eq!(
            query,
            "-in:drafts -in:spam -in:trash after:1790405999 before:1790492401"
        );
        assert!(!query.contains("in:inbox"));
        assert!(!query.contains("label:inbox"));
        assert!(!list_url.contains("in:inbox"));
        assert!(!list_url.contains("in%3Ainbox"));
    }

    #[test]
    fn maps_thread_across_two_civil_dates() {
        let value = read_fixture_json("thread-two-days.json");
        let messages = value.as_array().unwrap();
        let http = StaticHttp {
            status: 500,
            body: Vec::new(),
            requests: Mutex::new(Vec::new()),
        };
        let mut items: Vec<_> = messages
            .iter()
            .map(|message| map_fixture(message, "name@gmail.com", &http))
            .collect();
        assert_eq!(items.len(), 2);
        let m1 = items.iter().find(|item| item.tie_break == "m1").unwrap();
        let m2 = items.iter().find(|item| item.tie_break == "m2").unwrap();
        assert_eq!(m1.stable_id, "t1");
        assert_eq!(m2.stable_id, "t1");
        assert_eq!(m1.display_title, "Quarterly plan");
        assert_eq!(m2.display_title, "Re: Quarterly plan");
        assert_eq!(m1.file_day.to_string(), "2026-09-26");
        assert_eq!(m2.file_day.to_string(), "2026-09-27");
        assert_eq!(
            m1.t.format("%Y-%m-%d %H:%M:%S %z").to_string(),
            "2026-09-26 08:14:03 -0700"
        );
        assert_eq!(
            m2.t.format("%Y-%m-%d %H:%M:%S %z").to_string(),
            "2026-09-27 09:00:00 -0700"
        );
        assert_ne!(m1.file_day.to_string(), "2020-01-01");
        items.sort_by(|left, right| {
            left.t
                .timestamp_millis()
                .cmp(&right.t.timestamp_millis())
                .then_with(|| left.tie_break.as_bytes().cmp(right.tie_break.as_bytes()))
        });
        assert_eq!(items.last().unwrap().display_title, "Re: Quarterly plan");
    }

    #[test]
    fn internal_date_not_date_header() {
        let message = read_fixture_json("date-header-ignored.json");
        let http = StaticHttp {
            status: 500,
            body: Vec::new(),
            requests: Mutex::new(Vec::new()),
        };
        let item = map_fixture(&message, "name@gmail.com", &http);
        assert_eq!(item.file_day.to_string(), "2026-09-26");
        assert_eq!(
            item.t.format("%Y-%m-%d %H:%M:%S %z").to_string(),
            "2026-09-26 08:14:03 -0700"
        );
        let heading = item.body.lines().next().unwrap();
        assert!(heading.starts_with("## 2026-09-26 08:14:03 -0700"));
        assert!(!heading.contains("2020"));
        assert!(!item.body.contains("2020-01-01"));
    }

    #[test]
    fn slice_bounds_use_internal_date() {
        let start = midnight(2026, 9, 26);
        let end = midnight(2026, 9, 27);
        let start_ms = start.timestamp_millis();
        let end_ms = end.timestamp_millis();
        let messages = vec![
            tiny_message("before", start_ms - 1),
            tiny_message("start", start_ms),
            tiny_message("inside", end_ms - 1),
            tiny_message("end", end_ms),
        ];
        let vault = TempVault::new();
        let secrets = vault.root.join("secrets");
        write_secrets(&secrets);
        let http = FakeHttp::messages("name@gmail.com", messages);
        let items = fetch_with(
            &source_in(&secrets),
            &slice_between(start, end),
            &ContactIndex::empty(),
            &http,
        )
        .unwrap();
        let mut ids: Vec<_> = items.iter().map(|item| item.tie_break.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["inside", "start"]);
    }

    fn tiny_message(id: &str, internal_ms: i64) -> Value {
        serde_json::json!({
            "id": id,
            "threadId": "t-bounds",
            "internalDate": internal_ms.to_string(),
            "payload": {
                "mimeType": "text/plain",
                "headers": [
                    {"name": "Subject", "value": id},
                    {"name": "From", "value": "a@example.com"},
                    {"name": "To", "value": "name@gmail.com"}
                ],
                "body": {"data": "SGVsbG8"}
            }
        })
    }

    #[test]
    fn gmail_quote_html_to_markdown() {
        let html = fs::read_to_string(fixture("quote.html")).unwrap();
        let expected = fs::read_to_string(fixture("quote.md")).unwrap();
        let (markdown, cids) = html_to_markdown(&html);
        assert_eq!(markdown, expected);
        assert_eq!(cids, vec!["ii_abc".to_string()]);
        assert!(!markdown.contains("secret"));
        assert!(!markdown.contains("color:red"));
        assert!(!markdown.contains("cid:"));
    }

    #[test]
    fn cid_image_is_attachment_metadata_only() {
        let message = read_fixture_json("cid-message.json");
        let http = StaticHttp {
            status: 200,
            body: br#"{"data":"SGk"}"#.to_vec(),
            requests: Mutex::new(Vec::new()),
        };
        let item = map_fixture(&message, "name@gmail.com", &http);
        assert_eq!(item.attachments.len(), 3);
        assert_eq!(item.attachments[0].filename.as_deref(), Some("photo.png"));
        assert_eq!(item.attachments[0].media_type.as_deref(), Some("image/png"));
        assert_eq!(item.attachments[1].filename, None);
        assert_eq!(
            item.attachments[1].media_type.as_deref(),
            Some("application/pdf")
        );
        assert_eq!(item.attachments[2].filename, None);
        assert_eq!(item.attachments[2].media_type, None);
        assert!(!item.body.to_ascii_lowercase().contains("cid:"));
        let urls = http.requests.lock().unwrap().clone();
        assert!(urls.iter().all(|url| !url.contains("/attachments/")));
    }

    #[test]
    fn empty_subject() {
        let message = read_fixture_json("empty-subject.json");
        let http = StaticHttp {
            status: 500,
            body: Vec::new(),
            requests: Mutex::new(Vec::new()),
        };
        let item = map_fixture(&message, "name@gmail.com", &http);
        assert_eq!(item.display_title, "");
        assert_eq!(item.empty_title_fallback, "(no subject)");
        assert!(item.body.contains("- Subject: (no subject)"));
    }

    #[test]
    fn plain_text_when_html_missing_or_whitespace() {
        let http = StaticHttp {
            status: 500,
            body: Vec::new(),
            requests: Mutex::new(Vec::new()),
        };
        let plain = map_fixture(
            &plain_only_message("> quoted history\nHello"),
            "name@gmail.com",
            &http,
        );
        assert!(plain.body.contains("> quoted history\nHello"));
        assert!(plain.body.contains("- Subject: Plain note"));

        let fallback = map_fixture(
            &html_and_plain_message("<p>   </p>", "plain fallback line"),
            "name@gmail.com",
            &http,
        );
        assert!(fallback.body.contains("plain fallback line"));
        assert!(!fallback.body.contains("<p>"));

        let empty = map_fixture(
            &html_and_plain_message("<p> &nbsp; </p>", "   \n\t"),
            "name@gmail.com",
            &http,
        );
        assert!(empty.body.contains("- Subject: Still here"));
        assert!(empty.body.ends_with("- To: you@example.com\n"));
        assert!(!empty.body.contains("Still here\n\n"));
    }

    fn plain_only_message(text: &str) -> Value {
        serde_json::json!({
            "id": "plain-1",
            "threadId": "tp",
            "internalDate": "1790435643000",
            "payload": {
                "mimeType": "text/plain",
                "headers": [
                    {"name": "Subject", "value": "Plain note"},
                    {"name": "From", "value": "ada@example.com"},
                    {"name": "To", "value": "you@example.com"},
                    {"name": "Content-Type", "value": "text/plain; charset=UTF-8"}
                ],
                "body": {"data": b64(text)}
            }
        })
    }

    fn html_and_plain_message(html: &str, plain: &str) -> Value {
        serde_json::json!({
            "id": "both-1",
            "threadId": "tb",
            "internalDate": "1790435643000",
            "payload": {
                "mimeType": "multipart/alternative",
                "headers": [
                    {"name": "Subject", "value": "Still here"},
                    {"name": "From", "value": "ada@example.com"},
                    {"name": "To", "value": "you@example.com"}
                ],
                "parts": [
                    {
                        "mimeType": "text/plain",
                        "body": {"data": b64(plain)}
                    },
                    {
                        "mimeType": "text/html",
                        "body": {"data": b64(html)}
                    }
                ]
            }
        })
    }

    #[test]
    fn participants_and_me() {
        let vault = TempVault::new();
        let contacts_dir = vault.root.join("contacts");
        fs::create_dir_all(&contacts_dir).unwrap();
        fs::write(
            contacts_dir.join("Ada Lovelace.md"),
            "\
---
id: \"a\"
emails:
  - value: ada@example.com
---
",
        )
        .unwrap();
        let contacts = ContactIndex::load(&contacts_dir);
        let http = StaticHttp {
            status: 500,
            body: Vec::new(),
            requests: Mutex::new(Vec::new()),
        };
        let m1 = map_message(
            &participant_message(
                "m1",
                "1790435643000",
                "Ada B <Ada@example.com>",
                "You <you@example.com>, Robert <bob@example.com>",
                "Ada A <ada@example.com>",
                "Hello",
            ),
            "you@example.com",
            zone(),
            &contacts,
            &http,
            "access-token",
        )
        .unwrap();
        let m2 = map_message(
            &participant_message(
                "m2",
                "1790524800000",
                "Other Ada <ada@example.com>",
                "Bob <bob@example.com>",
                "",
                "Later",
            ),
            "you@example.com",
            zone(),
            &contacts,
            &http,
            "access-token",
        )
        .unwrap();
        let expected_m1 = "\
account: you@example.com
participants:
  - name: \"Ada Lovelace\"
    email: Ada@example.com
  - name: \"Robert\"
    email: bob@example.com
  - name: \"You\"
    email: you@example.com
";
        assert_eq!(m1.extra_front_matter, expected_m1);
        assert!(!m1.extra_front_matter.contains("[["));
        assert!(!m1.extra_front_matter.contains("Me"));
        let heading = m1.body.lines().next().unwrap();
        assert_eq!(
            heading,
            "## 2026-09-26 08:14:03 -0700 — [[Ada Lovelace]] <Ada@example.com>"
        );
        assert!(m1
            .body
            .contains("- To: You <you@example.com>, Robert <bob@example.com>"));
        assert!(m1.body.contains("- Cc: [[Ada Lovelace]] <ada@example.com>"));
        assert!(!m1.body.contains("- Bcc:"));
        let source = GmailSource {
            secrets_dir: vault.root.clone(),
            zone: zone(),
        };
        assert_eq!(
            source.group_extra_front_matter(std::slice::from_ref(&m1)),
            m1.extra_front_matter
        );
        let grouped = source.group_extra_front_matter(&[m1, m2]);
        assert_eq!(grouped, expected_m1);
        assert!(grouped.contains("name: \"Robert\""));
        assert!(!grouped.contains("Bob"));
        assert!(grouped.contains("ada@example.com") || grouped.contains("Ada@example.com"));
        assert!(grouped.contains("bob@example.com"));
        assert!(grouped.contains("you@example.com"));

        let me = map_message(
            &participant_message(
                "me1",
                "1790435643000",
                "Pat <you@example.com>",
                "ada@example.com",
                "",
                "Mine",
            ),
            "you@example.com",
            zone(),
            &contacts,
            &http,
            "access-token",
        )
        .unwrap();
        let me_heading = me.body.lines().next().unwrap();
        assert_eq!(me_heading, "## 2026-09-26 08:14:03 -0700 — Me");
        assert!(me.body.contains("- To: [[Ada Lovelace]] <ada@example.com>"));
        assert!(!me.body.contains("- To: Me"));
    }

    fn participant_message(
        id: &str,
        internal_date: &str,
        from: &str,
        to: &str,
        cc: &str,
        subject: &str,
    ) -> Value {
        let mut headers = vec![
            serde_json::json!({"name": "From", "value": from}),
            serde_json::json!({"name": "To", "value": to}),
            serde_json::json!({"name": "Subject", "value": subject}),
        ];
        if !cc.is_empty() {
            headers.push(serde_json::json!({"name": "Cc", "value": cc}));
        }
        serde_json::json!({
            "id": id,
            "threadId": "t-people",
            "internalDate": internal_date,
            "payload": {
                "mimeType": "text/plain",
                "headers": headers,
                "body": {"data": b64("Body")}
            }
        })
    }

    #[test]
    fn sent_message_maps() {
        let message = serde_json::json!({
            "id": "sent-1",
            "threadId": "t-sent",
            "labelIds": ["SENT"],
            "internalDate": "1790435643000",
            "payload": {
                "mimeType": "text/plain",
                "headers": [
                    {"name": "From", "value": "name@gmail.com"},
                    {"name": "To", "value": "ada@example.com"},
                    {"name": "Subject", "value": "Sent note"}
                ],
                "body": {"data": b64("Sent body")}
            }
        });
        let vault = TempVault::new();
        let secrets = vault.root.join("secrets");
        write_secrets(&secrets);
        let http = FakeHttp::messages("name@gmail.com", vec![message]);
        let start = midnight(2026, 9, 26);
        let end = midnight(2026, 9, 27);
        let items = fetch_with(
            &source_in(&secrets),
            &slice_between(start, end),
            &ContactIndex::empty(),
            &http,
        )
        .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].stable_id, "t-sent");
        assert_eq!(items[0].display_title, "Sent note");
        assert!(items[0].body.lines().next().unwrap().ends_with("— Me"));
    }

    #[test]
    fn source_url_encodes_account_not_thread_id() {
        assert_eq!(
            source_url("name@gmail.com", "18c2f0a1b2c3d4e5"),
            "https://mail.google.com/mail/?authuser=name%40gmail.com#all/18c2f0a1b2c3d4e5"
        );
        assert_eq!(
            source_url("name@gmail.com", "a+b"),
            "https://mail.google.com/mail/?authuser=name%40gmail.com#all/a+b"
        );
        assert_eq!(
            source_url("a+b@gmail.com", "thread"),
            "https://mail.google.com/mail/?authuser=a%2Bb%40gmail.com#all/thread"
        );
    }

    struct CallbackOpener {
        report: Arc<Mutex<String>>,
    }

    impl UrlOpener for CallbackOpener {
        fn open(&self, url: &str) -> bool {
            let url = url.to_string();
            let report = Arc::clone(&self.report);
            std::thread::spawn(move || {
                let result = hit_loopback(&url);
                *report.lock().unwrap() = result;
            });
            true
        }
    }

    fn hit_loopback(auth_url: &str) -> String {
        if !auth_url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?") {
            return format!("unexpected auth url {auth_url}");
        }
        if auth_url.contains("client_secret") || auth_url.contains("localhost") {
            return "auth url leaked a secret or used localhost".to_string();
        }
        let Some(redirect) = query_param(auth_url, "redirect_uri") else {
            return "missing redirect".to_string();
        };
        let Some(state) = query_param(auth_url, "state") else {
            return "missing state".to_string();
        };
        if !redirect.starts_with("http://127.0.0.1:") || !redirect.ends_with('/') {
            return format!("bad redirect {redirect}");
        }
        let port: u16 = redirect
            .trim_start_matches("http://127.0.0.1:")
            .trim_end_matches('/')
            .parse()
            .unwrap_or(0);
        if port == 0 {
            return "bad port".to_string();
        }
        let mut stream = None;
        for _ in 0..100 {
            if let Ok(connected) = TcpStream::connect(("127.0.0.1", port)) {
                stream = Some(connected);
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let Some(mut stream) = stream else {
            return "loopback did not accept".to_string();
        };
        let request = format!(
            "GET /?code=authcode&state={state} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        );
        if stream.write_all(request.as_bytes()).is_err() {
            return "write failed".to_string();
        }
        let mut response = String::new();
        if stream.read_to_string(&mut response).is_err() {
            return "read failed".to_string();
        }
        if !response.contains("HTTP/1.1 200") || !response.contains("text/html; charset=utf-8") {
            return format!("bad response status {response}");
        }
        if !response.contains(AUTH_OK_BODY) {
            return "missing confirmation".to_string();
        }
        if response.contains("authcode") || response.contains("refresh") {
            return "html contained a token".to_string();
        }
        "ok".to_string()
    }

    struct TokenOnlyHttp;

    impl GmailHttp for TokenOnlyHttp {
        fn execute(
            &self,
            method: &str,
            url: &str,
            headers: &[(&str, &str)],
            body: Option<&[u8]>,
        ) -> Result<HttpResponse, ()> {
            if method != "POST" || url != TOKEN_URL {
                return Err(());
            }
            let content_type = headers.iter().any(|(name, value)| {
                name.eq_ignore_ascii_case("Content-Type")
                    && *value == "application/x-www-form-urlencoded"
            });
            if !content_type {
                return Err(());
            }
            let form = String::from_utf8_lossy(body.unwrap_or_default());
            if !form.contains("grant_type=authorization_code") || !form.contains("code=authcode") {
                return Err(());
            }
            Ok(HttpResponse {
                status: 200,
                body: br#"{"access_token":"ya29.access","refresh_token":"1//new-refresh","leak":"super-secret-token-value"}"#.to_vec(),
            })
        }
    }

    #[test]
    fn authorize_stores_refresh_token_only() {
        let vault = TempVault::new();
        let secrets = vault.root.join("secrets");
        fs::create_dir_all(&secrets).unwrap();
        fs::write(
            secrets.join("gmail-client.json"),
            r#"{"client_id":"client-id","client_secret":"client-secret"}"#,
        )
        .unwrap();
        fs::write(
            secrets.join("gmail.json"),
            "{\"refresh_token\":\"old-refresh-token\"}\n",
        )
        .unwrap();
        let report = Arc::new(Mutex::new(String::from("pending")));
        let opener = CallbackOpener {
            report: Arc::clone(&report),
        };
        authorize_with(&secrets, &TokenOnlyHttp, &opener).unwrap();
        let started = Instant::now();
        loop {
            let status = report.lock().unwrap().clone();
            if status != "pending" {
                assert_eq!(status, "ok");
                break;
            }
            if started.elapsed() > Duration::from_secs(5) {
                panic!("opener did not finish");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let text = fs::read_to_string(secrets.join("gmail.json")).unwrap();
        assert_eq!(text, "{\"refresh_token\":\"1//new-refresh\"}\n");
        assert!(!text.contains("ya29.access"));
        assert!(!text.contains("access_token"));
        assert!(!text.contains("super-secret-token-value"));
        assert!(!text.contains("old-refresh-token"));
        assert!(!text.contains("client-secret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(secrets.join("gmail.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn error_reason_omits_response_body() {
        let secret = "super-secret-token-value";
        let sentence = "The quarterly plan body must stay out of the error.";
        let vault = TempVault::new();
        let secrets = vault.root.join("secrets");
        write_secrets(&secrets);
        let mut http = FakeHttp::messages("name@gmail.com", Vec::new());
        http.list_status = Some(500);
        http.list_body = Some(format!("{secret} {sentence}").into_bytes());
        let err = fetch_with(
            &source_in(&secrets),
            &slice_between(midnight(2026, 9, 26), midnight(2026, 9, 27)),
            &ContactIndex::empty(),
            &http,
        )
        .unwrap_err();
        let shown = err.to_string();
        assert_eq!(shown, "listing Gmail messages failed (HTTP 500)");
        assert!(!shown.contains(secret));
        assert!(!shown.contains(sentence));
        assert!(!shown.contains("quarterly"));
    }
}
