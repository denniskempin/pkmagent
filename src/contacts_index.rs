//! Wiki-link index built from `<vault>/contacts`. A missing directory is empty.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::note::{self, front_matter_scalar, yaml_list_values};

#[derive(Clone, Debug, Default)]
pub struct ContactIndex {
    emails: HashMap<String, Vec<String>>,
    phones: HashMap<String, Vec<String>>,
}

impl ContactIndex {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn load(dir: &Path) -> Self {
        if !dir.is_dir() {
            return Self::empty();
        }
        let mut index = Self::empty();
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return index,
        };
        for entry in entries.flatten() {
            let name = match entry.file_name().into_string() {
                Ok(name) => name,
                Err(_) => continue,
            };
            if !name.ends_with(".md") {
                continue;
            }
            let text = match fs::read_to_string(entry.path()) {
                Ok(text) => text,
                Err(_) => continue,
            };
            let Some((yaml, _)) = note::split_front_matter(&text) else {
                continue;
            };
            if front_matter_scalar(&yaml, "id").is_none() {
                continue;
            }
            let title = name.strip_suffix(".md").unwrap_or(&name).to_string();
            for email in yaml_list_values(&yaml, "emails") {
                let key = normalize_email(&email);
                if key.is_empty() {
                    continue;
                }
                index.emails.entry(key).or_default().push(title.clone());
            }
            for phone in yaml_list_values(&yaml, "phones") {
                let digits = digits_only(&phone);
                if digits.is_empty() {
                    continue;
                }
                index
                    .phones
                    .entry(phone_key(&digits))
                    .or_default()
                    .push(title.clone());
            }
        }
        index
    }

    pub fn title_for_email(&self, email: &str) -> Option<&str> {
        let key = normalize_email(email);
        if key.is_empty() {
            return None;
        }
        self.emails.get(&key).and_then(|titles| first_title(titles))
    }

    pub fn title_for_phone(&self, phone: &str) -> Option<&str> {
        let digits = digits_only(phone);
        if digits.is_empty() {
            return None;
        }
        self.phones
            .get(&phone_key(&digits))
            .and_then(|titles| first_title(titles))
    }
}

fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

fn digits_only(phone: &str) -> String {
    phone.chars().filter(|ch| ch.is_ascii_digit()).collect()
}

/// Equal digits, or a leading `1` on an 11-digit number whose tail is 10 digits.
fn phone_key(digits: &str) -> String {
    if digits.len() == 11 && digits.starts_with('1') {
        digits[1..].to_string()
    } else {
        digits.to_string()
    }
}

fn first_title(titles: &[String]) -> Option<&str> {
    titles
        .iter()
        .min_by(|left, right| left.as_bytes().cmp(right.as_bytes()))
        .map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempVault;

    fn write_contact(dir: &Path, name: &str, body: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn matches_email_and_phone_with_a_filename_tie_break() {
        let vault = TempVault::new();
        let dir = vault.root.join("contacts");
        write_contact(
            &dir,
            "Zoe.md",
            "\
---
id: \"z\"
emails:
  - value: Ada@Example.com
phones:
  - value: \"+1 555 121 2123\"
---
",
        );
        write_contact(
            &dir,
            "Ada Lovelace.md",
            "\
---
id: \"a\"
emails:
  - value: ada@example.com
phones:
  - value: 5551212123
---
",
        );
        write_contact(&dir, "notes.txt", "not a contact");
        write_contact(
            &dir,
            "No Id.md",
            "\
---
emails:
  - value: other@example.com
---
",
        );
        let index = ContactIndex::load(&dir);
        assert_eq!(
            index.title_for_email("  ADA@example.com "),
            Some("Ada Lovelace")
        );
        assert_eq!(
            index.title_for_email("ada@example.com"),
            Some("Ada Lovelace")
        );
        assert_eq!(index.title_for_phone("555-121-2123"), Some("Ada Lovelace"));
        assert_eq!(
            index.title_for_phone("+1 (555) 121-2123"),
            Some("Ada Lovelace")
        );
        assert_eq!(index.title_for_phone("25551212123"), None);
        assert_eq!(index.title_for_phone("555121212"), None);
        assert_eq!(index.title_for_email("other@example.com"), None);
        assert!(ContactIndex::load(vault.root.join("missing").as_path())
            .title_for_email("ada@example.com")
            .is_none());
    }
}
