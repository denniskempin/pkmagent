//! Markdown notes. Front matter is written by hand so the key order stays fixed.

use crate::sources::Attachment;

#[derive(Clone, Debug)]
pub struct NoteHeader {
    pub source: String,
    pub id: String,
    pub title: String,
    pub source_url: String,
    pub imported_at: String,
    pub day: Option<String>,
    pub timezone: Option<String>,
    pub extra_front_matter: String,
}

pub fn render_note(
    header: &NoteHeader,
    body_after_link: &str,
    attachments: &[Attachment],
) -> String {
    let mut text = render_front_matter(header);
    text.push_str(&source_link(&header.source, &header.source_url));
    text.push('\n');
    text.push_str(body_after_link);
    if !attachments.is_empty() {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        if !text.ends_with("\n\n") {
            text.push('\n');
        }
        text.push_str("### Attachments\n");
        for attachment in attachments {
            let name = attachment
                .filename
                .as_deref()
                .filter(|name| !name.is_empty())
                .unwrap_or("unnamed");
            match attachment
                .media_type
                .as_deref()
                .filter(|kind| !kind.is_empty())
            {
                Some(kind) => text.push_str(&format!("- {name} ({kind})\n")),
                None => text.push_str(&format!("- {name}\n")),
            }
        }
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

pub fn render_front_matter(header: &NoteHeader) -> String {
    let mut text = String::from("---\n");
    text.push_str(&format!("source: {}\n", header.source));
    text.push_str(&format!("id: {}\n", yaml_double_quoted(&header.id)));
    text.push_str(&format!("title: {}\n", yaml_double_quoted(&header.title)));
    text.push_str(&format!(
        "source_url: {}\n",
        yaml_double_quoted(&header.source_url)
    ));
    text.push_str(&format!("imported_at: {}\n", header.imported_at));
    if let Some(day) = &header.day {
        text.push_str(&format!("day: {day}\n"));
    }
    if let Some(timezone) = &header.timezone {
        text.push_str(&format!("timezone: {timezone}\n"));
    }
    if !header.extra_front_matter.is_empty() {
        text.push_str(&header.extra_front_matter);
        if !header.extra_front_matter.ends_with('\n') {
            text.push('\n');
        }
    }
    text.push_str("---\n");
    text
}

pub fn source_link(source: &str, url: &str) -> String {
    let label = match source {
        "gmail" => "Open in Gmail",
        "messages" => "Open in Messages",
        "calendar" => "Open in Calendar",
        "contacts" => "Open in Contacts",
        _ => "Open",
    };
    format!("[{label}]({url})")
}

pub fn yaml_double_quoted(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u00{:02X}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Split a note into the front-matter block (through the newline after the closing `---`)
/// and the body bytes that follow it.
pub fn split_front_matter(text: &str) -> Option<(String, String)> {
    let rest = if let Some(rest) = text.strip_prefix("---\r\n") {
        rest
    } else if let Some(rest) = text.strip_prefix("---\n") {
        rest
    } else {
        return None;
    };
    let bytes = rest.as_bytes();
    let mut index = 0;
    let mut line_start = 0;
    while index <= bytes.len() {
        if index == bytes.len() || bytes[index] == b'\n' {
            let mut line = &rest[line_start..index];
            if let Some(stripped) = line.strip_suffix('\r') {
                line = stripped;
            }
            if line == "---" {
                let body_start = if index < bytes.len() {
                    index + 1
                } else {
                    index
                };
                let mut yaml = &rest[..line_start];
                if let Some(stripped) = yaml.strip_suffix('\n') {
                    yaml = stripped;
                }
                if let Some(stripped) = yaml.strip_suffix('\r') {
                    yaml = stripped;
                }
                return Some((yaml.to_string(), rest[body_start..].to_string()));
            }
            if index == bytes.len() {
                break;
            }
            index += 1;
            line_start = index;
            continue;
        }
        index += 1;
    }
    None
}

pub fn front_matter_scalar(yaml: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    for line in yaml.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        if let Some(rest) = line.strip_prefix(&prefix) {
            let value = parse_scalar(rest);
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

pub fn yaml_list_values(yaml: &str, section: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut active = false;
    for line in yaml.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let nested = line.starts_with(' ') || line.starts_with('\t');
        if !nested {
            let key = line.split(':').next().unwrap_or("").trim();
            active = key == section;
            continue;
        }
        if !active {
            continue;
        }
        let trimmed = line.trim();
        let trimmed = trimmed.strip_prefix("- ").unwrap_or(trimmed).trim();
        if let Some(rest) = trimmed.strip_prefix("value:") {
            values.push(parse_scalar(rest));
        }
    }
    values
}

pub fn parse_scalar(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        return unescape_double(&value[1..value.len() - 1]);
    }
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return value[1..value.len() - 1].to_string();
    }
    value.to_string()
}

fn unescape_double(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Keep the existing body and replace the front matter. `rendered_note` is a full note;
/// only its front matter is used.
#[cfg_attr(not(test), allow(dead_code))]
pub fn replace_front_matter(existing: &str, rendered_note: &str) -> Result<String, String> {
    let (_yaml, body) =
        split_front_matter(existing).ok_or_else(|| "front matter missing".to_string())?;
    let (_new_yaml, rendered_body) =
        split_front_matter(rendered_note).ok_or_else(|| "front matter missing".to_string())?;
    if !rendered_note.ends_with(&rendered_body) {
        return Err("front matter missing".to_string());
    }
    let prefix_len = rendered_note.len() - rendered_body.len();
    Ok(format!("{}{body}", &rendered_note[..prefix_len]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(source: &str, extra: &str, day: Option<&str>, timezone: Option<&str>) -> NoteHeader {
        NoteHeader {
            source: source.into(),
            id: "abc".into(),
            title: "Quarterly plan".into(),
            source_url: "https://example.test/abc".into(),
            imported_at: "2026-09-27T10:00:00-07:00".into(),
            day: day.map(str::to_string),
            timezone: timezone.map(str::to_string),
            extra_front_matter: extra.into(),
        }
    }

    #[test]
    fn collect_front_matter_keeps_key_order() {
        let note = render_note(
            &header(
                "gmail",
                "account: me@example.com\n",
                Some("2026-09-26"),
                Some("America/Los_Angeles"),
            ),
            "\nHello.\n",
            &[
                Attachment {
                    filename: Some("plan.pdf".into()),
                    media_type: Some("application/pdf".into()),
                },
                Attachment {
                    filename: None,
                    media_type: None,
                },
            ],
        );
        let expected = "\
---
source: gmail
id: \"abc\"
title: \"Quarterly plan\"
source_url: \"https://example.test/abc\"
imported_at: 2026-09-27T10:00:00-07:00
day: 2026-09-26
timezone: America/Los_Angeles
account: me@example.com
---
[Open in Gmail](https://example.test/abc)

Hello.

### Attachments
- plan.pdf (application/pdf)
- unnamed
";
        assert_eq!(note, expected);
        let quoted = render_note(
            &NoteHeader {
                title: "Say \"hi\"".into(),
                ..header(
                    "messages",
                    "",
                    Some("2026-09-26"),
                    Some("America/Los_Angeles"),
                )
            },
            "\n",
            &[],
        );
        assert!(quoted.contains("title: \"Say \\\"hi\\\"\""));
        assert!(quoted.contains("[Open in Messages]("));
    }

    #[test]
    fn contact_files_omit_day_and_timezone() {
        let fresh = render_note(
            &header("contacts", "phones:\n  - label: mobile\n", None, None),
            "\nNote text.",
            &[],
        );
        assert!(fresh.contains("source: contacts\n"));
        assert!(!fresh.contains("\nday:"));
        assert!(!fresh.contains("timezone:"));
        assert!(fresh.contains("[Open in Contacts](https://example.test/abc)\n\nNote text.\n"));
        let empty = render_note(&header("contacts", "", None, None), "", &[]);
        assert!(empty.ends_with("[Open in Contacts](https://example.test/abc)\n"));
        assert!(!empty.ends_with("\n\n"));
    }

    #[test]
    fn refreshed_contact_keeps_body_and_imported_at() {
        let existing = "\
---
source: contacts
id: \"abc\"
title: \"Custom Name\"
source_url: \"addressbook://abc\"
imported_at: 2020-01-02T03:04:05-07:00
---
edited body";
        let rendered = render_note(
            &NoteHeader {
                source: "contacts".into(),
                id: "abc".into(),
                title: "Custom Name".into(),
                source_url: "addressbook://abc".into(),
                imported_at: "2020-01-02T03:04:05-07:00".into(),
                day: None,
                timezone: None,
                extra_front_matter: "phones:\n  - label: \"mobile\"\n    value: \"+1\"\n".into(),
            },
            "",
            &[],
        );
        let updated = replace_front_matter(existing, &rendered).unwrap();
        let (yaml, body) = split_front_matter(&updated).unwrap();
        assert_eq!(body, "edited body");
        assert!(!body.contains("imported_at"));
        assert_eq!(
            front_matter_scalar(&yaml, "imported_at").as_deref(),
            Some("2020-01-02T03:04:05-07:00")
        );
        assert!(yaml.contains("phones:"));
        assert!(!yaml.contains("\nday:"));
        assert!(!yaml.contains("timezone:"));
        assert!(replace_front_matter("no front matter", &rendered).is_err());
    }
}
