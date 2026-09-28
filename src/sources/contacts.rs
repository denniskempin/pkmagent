//! Contacts import. Fetch is macOS-only. Create, refresh, and delete run on every target.

#![cfg_attr(all(not(test), not(target_os = "macos")), allow(dead_code))]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::filename::allocate_filename;
use crate::note::{self, front_matter_scalar, render_note, yaml_double_quoted, NoteHeader};
use crate::sources::SourceError;

#[derive(Clone, Debug)]
pub struct ContactRecord {
    pub id: String,
    pub display_title: String,
    pub source_url: String,
    pub extra_front_matter: String,
    pub note_body: String,
}

struct LabeledValue {
    label: String,
    value: String,
}

struct ContactFields {
    id: String,
    display_name: String,
    is_organization: bool,
    organization: String,
    job_title: String,
    birthday: Option<String>,
    phones: Vec<LabeledValue>,
    emails: Vec<LabeledValue>,
    addresses: Vec<LabeledValue>,
    urls: Vec<LabeledValue>,
    note: String,
}

struct ExistingFile {
    name: String,
    text: String,
    id: Option<String>,
}

struct Scan {
    names: Vec<String>,
    files: Vec<ExistingFile>,
}

pub fn fetch_snapshot() -> Result<Vec<ContactRecord>, SourceError> {
    #[cfg(target_os = "macos")]
    {
        apple::fetch_snapshot()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(SourceError::new("Contacts is only available on macOS"))
    }
}

pub fn import_contacts(
    contacts_dir: &Path,
    snapshot: Result<Vec<ContactRecord>, SourceError>,
    imported_at: &str,
    progress: &mut dyn FnMut(u64, u64, &str),
) -> Result<u64, SourceError> {
    let mut records = snapshot?;
    fs::create_dir_all(contacts_dir).map_err(write_failed)?;
    records.sort_by(|left, right| left.id.as_bytes().cmp(right.id.as_bytes()));

    let scan = scan_dir(contacts_dir)?;
    let chosen = chosen_indexes(&scan.files);
    let mut taken = scan.names.clone();
    let total = records.len() as u64;
    let mut done = 0u64;
    let mut count = 0u64;

    for record in &records {
        let label = progress_label(&record.display_title);
        if let Some(index) = chosen.get(&record.id).copied() {
            let file = &scan.files[index];
            let title = title_from_filename(&file.name);
            let bytes = render_refresh(&file.text, record, &title, imported_at)?;
            write_bytes(&contacts_dir.join(&file.name), &bytes)?;
        } else {
            let name = allocate_filename(&record.display_title, "untitled", &record.id, &taken);
            let title = title_from_filename(&name);
            let bytes = render_created(record, &title, imported_at);
            write_bytes(&contacts_dir.join(&name), &bytes)?;
            taken.push(name);
        }
        done += 1;
        count += 1;
        progress(done, total, label);
    }

    let ids: HashSet<&str> = records.iter().map(|record| record.id.as_str()).collect();
    for file in &scan.files {
        if let Some(id) = &file.id {
            if !ids.contains(id.as_str()) {
                fs::remove_file(contacts_dir.join(&file.name)).map_err(write_failed)?;
            }
        }
    }
    Ok(count)
}

pub fn source_url(id: &str) -> String {
    let mut out = String::from("addressbook://");
    for byte in id.as_bytes() {
        if is_unreserved(*byte) {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub fn format_address(
    street: &str,
    sub_locality: &str,
    city: &str,
    sub_administrative_area: &str,
    state: &str,
    postal_code: &str,
    country: &str,
) -> String {
    let mut parts = Vec::new();
    for component in [
        street,
        sub_locality,
        city,
        sub_administrative_area,
        state,
        postal_code,
        country,
    ] {
        let cleaned = clean_component(component);
        if !cleaned.is_empty() {
            parts.push(cleaned);
        }
    }
    parts.join(", ")
}

pub fn format_birthday(year: Option<i32>, month: i32, day: i32) -> Option<String> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    match year {
        Some(year) if year >= 0 => Some(format!("{year:04}-{month:02}-{day:02}")),
        Some(_) => None,
        None => Some(format!("--{month:02}-{day:02}")),
    }
}

fn build_record(fields: ContactFields) -> Option<ContactRecord> {
    if fields.id.is_empty() {
        return None;
    }
    let phones = sorted_items(fields.phones);
    let emails = sorted_items(fields.emails);
    let addresses = sorted_items(fields.addresses);
    let urls = sorted_items(fields.urls);
    Some(ContactRecord {
        display_title: choose_display_title(
            &fields.display_name,
            fields.is_organization,
            &fields.organization,
        ),
        source_url: source_url(&fields.id),
        extra_front_matter: render_extra(
            &fields.organization,
            &fields.job_title,
            fields.birthday.as_deref(),
            &phones,
            &emails,
            &addresses,
            &urls,
        ),
        note_body: note_body(&fields.note),
        id: fields.id,
    })
}

fn choose_display_title(display_name: &str, is_organization: bool, organization: &str) -> String {
    let collapsed_name = collapse_whitespace(display_name);
    if !collapsed_name.is_empty() {
        return collapsed_name;
    }
    if is_organization {
        let collapsed_org = collapse_whitespace(organization);
        if !collapsed_org.is_empty() {
            return collapsed_org;
        }
    }
    String::new()
}

fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn note_body(note: &str) -> String {
    let trimmed = note.trim_end_matches(['\n', '\r']);
    if trimmed.trim().is_empty() {
        String::new()
    } else {
        format!("\n{trimmed}")
    }
}

fn sorted_items(mut items: Vec<LabeledValue>) -> Vec<LabeledValue> {
    items.retain(|item| !item.value.is_empty());
    items.sort_by(|left, right| {
        left.label
            .cmp(&right.label)
            .then_with(|| left.value.cmp(&right.value))
    });
    items
}

fn render_extra(
    organization: &str,
    job_title: &str,
    birthday: Option<&str>,
    phones: &[LabeledValue],
    emails: &[LabeledValue],
    addresses: &[LabeledValue],
    urls: &[LabeledValue],
) -> String {
    let mut out = String::new();
    push_scalar(&mut out, "organization", organization);
    push_scalar(&mut out, "job_title", job_title);
    if let Some(birthday) = birthday {
        push_scalar(&mut out, "birthday", birthday);
    }
    push_list(&mut out, "phones", phones);
    push_list(&mut out, "emails", emails);
    push_list(&mut out, "addresses", addresses);
    push_list(&mut out, "urls", urls);
    out
}

fn push_scalar(out: &mut String, key: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    out.push_str(key);
    out.push_str(": ");
    out.push_str(&yaml_double_quoted(value));
    out.push('\n');
}

fn push_list(out: &mut String, key: &str, items: &[LabeledValue]) {
    if items.is_empty() {
        return;
    }
    out.push_str(key);
    out.push_str(":\n");
    for item in items {
        out.push_str("  - label: ");
        out.push_str(&yaml_double_quoted(&item.label));
        out.push('\n');
        out.push_str("    value: ");
        out.push_str(&yaml_double_quoted(&item.value));
        out.push('\n');
    }
}

fn progress_label(display_title: &str) -> &str {
    if display_title.is_empty() {
        "untitled"
    } else {
        display_title
    }
}

fn title_from_filename(name: &str) -> String {
    name.strip_suffix(".md").unwrap_or(name).to_string()
}

fn contact_header(record: &ContactRecord, title: &str, imported_at: &str) -> NoteHeader {
    NoteHeader {
        source: "contacts".to_string(),
        id: record.id.clone(),
        title: title.to_string(),
        source_url: record.source_url.clone(),
        imported_at: imported_at.to_string(),
        day: None,
        timezone: None,
        extra_front_matter: record.extra_front_matter.clone(),
    }
}

fn render_created(record: &ContactRecord, title: &str, imported_at: &str) -> String {
    render_note(
        &contact_header(record, title, imported_at),
        &record.note_body,
        &[],
    )
}

fn render_refresh(
    existing: &str,
    record: &ContactRecord,
    title: &str,
    frozen: &str,
) -> Result<String, SourceError> {
    let imported_at = preserved_imported_at(existing, frozen);
    let rendered = render_note(&contact_header(record, title, &imported_at), "", &[]);
    note::replace_front_matter(existing, &rendered).map_err(write_failed)
}

fn preserved_imported_at(existing: &str, frozen: &str) -> String {
    note::split_front_matter(existing)
        .and_then(|(yaml, _)| front_matter_scalar(&yaml, "imported_at"))
        .unwrap_or_else(|| frozen.to_string())
}

fn scan_dir(dir: &Path) -> Result<Scan, SourceError> {
    let mut names = Vec::new();
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).map_err(write_failed)? {
        let entry = entry.map_err(write_failed)?;
        let name = match entry.file_name().into_string() {
            Ok(name) => name,
            Err(_) => continue,
        };
        names.push(name.clone());
        if !name.ends_with(".md") {
            continue;
        }
        let file_type = entry.file_type().map_err(write_failed)?;
        if !file_type.is_file() {
            continue;
        }
        let text = match fs::read_to_string(entry.path()) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::InvalidData => continue,
            Err(err) => return Err(write_failed(err)),
        };
        let id =
            note::split_front_matter(&text).and_then(|(yaml, _)| front_matter_scalar(&yaml, "id"));
        files.push(ExistingFile { name, text, id });
    }
    Ok(Scan { names, files })
}

fn chosen_indexes(files: &[ExistingFile]) -> HashMap<String, usize> {
    let mut best: HashMap<String, usize> = HashMap::new();
    for (index, file) in files.iter().enumerate() {
        let Some(id) = &file.id else {
            continue;
        };
        match best.get(id) {
            Some(prev) if files[*prev].name.as_bytes() <= file.name.as_bytes() => {}
            _ => {
                best.insert(id.clone(), index);
            }
        }
    }
    best
}

fn write_bytes(path: &Path, bytes: &str) -> Result<(), SourceError> {
    fs::write(path, bytes).map_err(write_failed)
}

fn write_failed(err: impl std::fmt::Display) -> SourceError {
    SourceError::new(format!("write failed: {err}"))
}

fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

fn clean_component(component: &str) -> String {
    component
        .replace("\r\n", ", ")
        .replace('\n', ", ")
        .replace('\r', ", ")
        .trim_matches(' ')
        .to_string()
}

#[cfg(target_os = "macos")]
mod apple {
    use std::cell::RefCell;
    use std::ptr::NonNull;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, ProtocolObject};
    use objc2::AnyThread;
    use objc2::Message;
    use objc2_contacts::{
        CNAuthorizationStatus, CNContact, CNContactBirthdayKey, CNContactEmailAddressesKey,
        CNContactFetchRequest, CNContactFormatter, CNContactFormatterStyle, CNContactIdentifierKey,
        CNContactJobTitleKey, CNContactNoteKey, CNContactOrganizationNameKey,
        CNContactPhoneNumbersKey, CNContactPostalAddressesKey, CNContactSortOrder, CNContactStore,
        CNContactType, CNContactTypeKey, CNContactUrlAddressesKey, CNEntityType, CNErrorCode,
        CNKeyDescriptor, CNLabeledValue, CNPhoneNumber, CNPostalAddress,
    };
    use objc2_foundation::{
        NSArray, NSCopying, NSDateComponentUndefined, NSDateComponents, NSError, NSInteger,
        NSSecureCoding, NSString,
    };

    use super::{build_record, ContactFields, ContactRecord, LabeledValue, SourceError};

    pub fn fetch_snapshot() -> Result<Vec<ContactRecord>, SourceError> {
        objc2::rc::autoreleasepool(|_| fetch_inside())
    }

    fn fetch_inside() -> Result<Vec<ContactRecord>, SourceError> {
        let status =
            unsafe { CNContactStore::authorizationStatusForEntityType(CNEntityType::Contacts) };
        if status == CNAuthorizationStatus::Denied {
            return Err(SourceError::new("access denied"));
        }
        if status == CNAuthorizationStatus::Restricted {
            return Err(SourceError::new("access restricted"));
        }

        let store = unsafe { CNContactStore::new() };
        let keys = contact_keys();
        let request = unsafe {
            let request =
                CNContactFetchRequest::initWithKeysToFetch(CNContactFetchRequest::alloc(), &keys);
            request.setPredicate(None);
            request.setUnifyResults(true);
            request.setMutableObjects(false);
            request.setSortOrder(CNContactSortOrder::None);
            request
        };

        let records = RefCell::new(Vec::new());
        let mut error: Option<Retained<NSError>> = None;
        let succeeded = {
            let block = RcBlock::new(|contact: NonNull<CNContact>, _stop: NonNull<Bool>| {
                let contact = unsafe { contact.as_ref() };
                if let Some(record) = map_contact(contact) {
                    records.borrow_mut().push(record);
                }
            });
            unsafe {
                store.enumerateContactsWithFetchRequest_error_usingBlock(
                    &request,
                    Some(&mut error),
                    &block,
                )
            }
        };
        if !succeeded {
            return Err(store_error(error));
        }
        Ok(dedupe_ids(records.into_inner()))
    }

    fn contact_keys() -> Retained<NSArray<ProtocolObject<dyn CNKeyDescriptor>>> {
        let name = unsafe {
            CNContactFormatter::descriptorForRequiredKeysForStyle(CNContactFormatterStyle::FullName)
        };
        let id_key = ProtocolObject::from_ref(unsafe { CNContactIdentifierKey });
        let type_key = ProtocolObject::from_ref(unsafe { CNContactTypeKey });
        let org_key = ProtocolObject::from_ref(unsafe { CNContactOrganizationNameKey });
        let job_key = ProtocolObject::from_ref(unsafe { CNContactJobTitleKey });
        let birthday_key = ProtocolObject::from_ref(unsafe { CNContactBirthdayKey });
        let phone_key = ProtocolObject::from_ref(unsafe { CNContactPhoneNumbersKey });
        let email_key = ProtocolObject::from_ref(unsafe { CNContactEmailAddressesKey });
        let address_key = ProtocolObject::from_ref(unsafe { CNContactPostalAddressesKey });
        let url_key = ProtocolObject::from_ref(unsafe { CNContactUrlAddressesKey });
        let note_key = ProtocolObject::from_ref(unsafe { CNContactNoteKey });
        NSArray::from_slice(&[
            &*name,
            id_key,
            type_key,
            org_key,
            job_key,
            birthday_key,
            phone_key,
            email_key,
            address_key,
            url_key,
            note_key,
        ])
    }

    fn map_contact(contact: &CNContact) -> Option<ContactRecord> {
        let id = unsafe { contact.identifier() }.to_string();
        let display_name = unsafe {
            CNContactFormatter::stringFromContact_style(contact, CNContactFormatterStyle::FullName)
        }
        .map(|name| name.to_string())
        .unwrap_or_default();
        let is_organization = unsafe { contact.contactType() } == CNContactType::Organization;
        let organization = unsafe { contact.organizationName() }.to_string();
        let job_title = unsafe { contact.jobTitle() }.to_string();
        let birthday =
            unsafe { contact.birthday() }.and_then(|components| birthday_text(&components));
        let phone_numbers = unsafe { contact.phoneNumbers() };
        let email_addresses = unsafe { contact.emailAddresses() };
        let postal_addresses = unsafe { contact.postalAddresses() };
        let url_addresses = unsafe { contact.urlAddresses() };
        let phones = labeled_phones(&phone_numbers);
        let emails = labeled_strings(&email_addresses);
        let addresses = labeled_addresses(&postal_addresses);
        let urls = labeled_strings(&url_addresses);
        let note = unsafe { contact.note() }.to_string();
        build_record(ContactFields {
            id,
            display_name,
            is_organization,
            organization,
            job_title,
            birthday,
            phones,
            emails,
            addresses,
            urls,
            note,
        })
    }

    fn labeled_phones(values: &NSArray<CNLabeledValue<CNPhoneNumber>>) -> Vec<LabeledValue> {
        values
            .iter()
            .map(|labeled| LabeledValue {
                label: localized_label(&labeled),
                value: unsafe { labeled.value().stringValue() }.to_string(),
            })
            .collect()
    }

    fn labeled_strings(values: &NSArray<CNLabeledValue<NSString>>) -> Vec<LabeledValue> {
        values
            .iter()
            .map(|labeled| LabeledValue {
                label: localized_label(&labeled),
                value: unsafe { labeled.value() }.to_string(),
            })
            .collect()
    }

    fn labeled_addresses(values: &NSArray<CNLabeledValue<CNPostalAddress>>) -> Vec<LabeledValue> {
        values
            .iter()
            .map(|labeled| {
                let address = unsafe { labeled.value() };
                let value = unsafe {
                    super::format_address(
                        &address.street().to_string(),
                        &address.subLocality().to_string(),
                        &address.city().to_string(),
                        &address.subAdministrativeArea().to_string(),
                        &address.state().to_string(),
                        &address.postalCode().to_string(),
                        &address.country().to_string(),
                    )
                };
                LabeledValue {
                    label: localized_label(&labeled),
                    value,
                }
            })
            .collect()
    }

    fn localized_label<T>(labeled: &CNLabeledValue<T>) -> String
    where
        T: Message + NSCopying + NSSecureCoding,
    {
        match unsafe { labeled.label() } {
            Some(label) => {
                unsafe { CNLabeledValue::<T>::localizedStringForLabel(&label) }.to_string()
            }
            None => String::new(),
        }
    }

    fn birthday_text(components: &NSDateComponents) -> Option<String> {
        let month = defined_component(components.month())?;
        let day = defined_component(components.day())?;
        let year_raw = components.year();
        let year = if year_raw == NSDateComponentUndefined {
            None
        } else {
            Some(i32::try_from(year_raw).ok()?)
        };
        super::format_birthday(year, month, day)
    }

    fn defined_component(value: NSInteger) -> Option<i32> {
        if value == NSDateComponentUndefined {
            None
        } else {
            i32::try_from(value).ok()
        }
    }

    fn store_error(error: Option<Retained<NSError>>) -> SourceError {
        let Some(error) = error else {
            return SourceError::new("fetch failed");
        };
        if error.code() == CNErrorCode::AuthorizationDenied.0
            && error.domain().to_string() == "CNErrorDomain"
        {
            return SourceError::new("access denied");
        }
        SourceError::new(format!(
            "fetch failed ({} {})",
            error.domain(),
            error.code()
        ))
    }

    fn dedupe_ids(mut records: Vec<ContactRecord>) -> Vec<ContactRecord> {
        records.sort_by(|left, right| left.id.as_bytes().cmp(right.id.as_bytes()));
        let mut out = Vec::with_capacity(records.len());
        for record in records {
            if out
                .last()
                .is_some_and(|previous: &ContactRecord| previous.id == record.id)
            {
                continue;
            }
            out.push(record);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::split_front_matter;
    use crate::testutil::TempVault;
    use serde::Deserialize;
    use std::path::PathBuf;

    #[derive(Deserialize)]
    struct Fixture {
        identifier: String,
        contact_type: String,
        #[serde(default)]
        display_name: String,
        #[serde(default)]
        organization: String,
        #[serde(default)]
        job_title: String,
        #[serde(default)]
        birthday: Option<FixtureBirthday>,
        #[serde(default)]
        phones: Vec<FixtureLabeled>,
        #[serde(default)]
        emails: Vec<FixtureLabeled>,
        #[serde(default)]
        addresses: Vec<FixtureAddress>,
        #[serde(default)]
        urls: Vec<FixtureLabeled>,
        #[serde(default)]
        note: Option<String>,
    }

    #[derive(Deserialize)]
    struct FixtureBirthday {
        year: Option<i32>,
        month: i32,
        day: i32,
    }

    #[derive(Deserialize)]
    struct FixtureLabeled {
        label: String,
        value: String,
    }

    #[derive(Deserialize)]
    struct FixtureAddress {
        label: String,
        street: String,
        sub_locality: String,
        city: String,
        sub_administrative_area: String,
        state: String,
        postal_code: String,
        country: String,
    }

    fn load_fixture(name: &str) -> ContactRecord {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/contacts")
            .join(name);
        let text = fs::read_to_string(path).unwrap();
        let fixture: Fixture = serde_json::from_str(&text).unwrap();
        let birthday = fixture
            .birthday
            .and_then(|birthday| format_birthday(birthday.year, birthday.month, birthday.day));
        let addresses = fixture
            .addresses
            .into_iter()
            .map(|address| LabeledValue {
                label: address.label,
                value: format_address(
                    &address.street,
                    &address.sub_locality,
                    &address.city,
                    &address.sub_administrative_area,
                    &address.state,
                    &address.postal_code,
                    &address.country,
                ),
            })
            .collect();
        build_record(ContactFields {
            id: fixture.identifier,
            display_name: fixture.display_name,
            is_organization: fixture.contact_type == "organization",
            organization: fixture.organization,
            job_title: fixture.job_title,
            birthday,
            phones: fixture
                .phones
                .into_iter()
                .map(|item| LabeledValue {
                    label: item.label,
                    value: item.value,
                })
                .collect(),
            emails: fixture
                .emails
                .into_iter()
                .map(|item| LabeledValue {
                    label: item.label,
                    value: item.value,
                })
                .collect(),
            addresses,
            urls: fixture
                .urls
                .into_iter()
                .map(|item| LabeledValue {
                    label: item.label,
                    value: item.value,
                })
                .collect(),
            note: fixture.note.unwrap_or_default(),
        })
        .expect("fixture identifier")
    }

    fn ada_id() -> &'static str {
        "00000000-0000-0000-0000-000000000001"
    }

    #[test]
    fn fixture_ada_front_matter_and_body() {
        let record = load_fixture("ada.json");
        let extra = "\
organization: \"Analytical Engines\"
job_title: \"Mathematician\"
birthday: \"1815-12-10\"
phones:
  - label: \"mobile\"
    value: \"+15551212\"
emails:
  - label: \"home\"
    value: \"ada@example.com\"
addresses:
  - label: \"work\"
    value: \"123 Main St, London\"
urls:
  - label: \"homepage\"
    value: \"https://example.com\"
";
        assert_eq!(record.extra_front_matter, extra);
        assert!(!record.extra_front_matter.contains("Note text"));
        assert!(!record.extra_front_matter.contains("\nnote:"));
        let file = render_created(&record, "Ada Lovelace", "2026-09-27T18:04:11-07:00");
        let expected = "\
---
source: contacts
id: \"00000000-0000-0000-0000-000000000001\"
title: \"Ada Lovelace\"
source_url: \"addressbook://00000000-0000-0000-0000-000000000001\"
imported_at: 2026-09-27T18:04:11-07:00
organization: \"Analytical Engines\"
job_title: \"Mathematician\"
birthday: \"1815-12-10\"
phones:
  - label: \"mobile\"
    value: \"+15551212\"
emails:
  - label: \"home\"
    value: \"ada@example.com\"
addresses:
  - label: \"work\"
    value: \"123 Main St, London\"
urls:
  - label: \"homepage\"
    value: \"https://example.com\"
---
[Open in Contacts](addressbook://00000000-0000-0000-0000-000000000001)

Note text.
";
        assert_eq!(file, expected);
    }

    #[test]
    fn lists_sorted_and_values_unchanged() {
        let record = load_fixture("unsorted.json");
        let extra = "\
phones:
  - label: \"home\"
    value: \"+0\"
  - label: \"mobile\"
    value: \"+1\"
  - label: \"mobile\"
    value: \"+2\"
  - label: \"work\"
    value: \"+3\"
emails:
  - label: \"home\"
    value: \"Ada@Example.com\"
";
        assert_eq!(record.extra_front_matter, extra);
        assert!(!record.extra_front_matter.contains("organization:"));
        assert!(!record.extra_front_matter.contains("job_title:"));
    }

    #[test]
    fn omit_empty_note() {
        let record = build_record(ContactFields {
            id: "space".into(),
            display_name: "Ada".into(),
            is_organization: false,
            organization: String::new(),
            job_title: String::new(),
            birthday: None,
            phones: Vec::new(),
            emails: Vec::new(),
            addresses: Vec::new(),
            urls: Vec::new(),
            note: "   ".into(),
        })
        .unwrap();
        assert!(record.note_body.is_empty());
        let file = render_created(&record, "Ada", "2026-09-27T18:04:11-07:00");
        assert!(file.ends_with("[Open in Contacts](addressbook://space)\n"));
        assert!(!file.ends_with("\n\n"));
    }

    #[test]
    fn source_url_percent_encodes_identifier() {
        assert_eq!(source_url("ABC DEF/1"), "addressbook://ABC%20DEF%2F1");
    }

    #[test]
    fn import_create() {
        let vault = TempVault::new();
        let dir = vault.root.join("contacts");
        let frozen = "2026-09-27T10:00:00-07:00";
        let count = import_contacts(
            &dir,
            Ok(vec![load_fixture("ada.json")]),
            frozen,
            &mut |_, _, _| {},
        )
        .unwrap();
        assert_eq!(count, 1);
        assert!(dir.is_dir());
        let text = fs::read_to_string(dir.join("Ada Lovelace.md")).unwrap();
        let (yaml, _) = split_front_matter(&text).unwrap();
        assert_eq!(
            front_matter_scalar(&yaml, "imported_at").as_deref(),
            Some(frozen)
        );
        assert_eq!(
            front_matter_scalar(&yaml, "title").as_deref(),
            Some("Ada Lovelace")
        );
        assert!(!vault.inbox().exists());
        assert!(!vault.root.join(".pkmagent").join("state.json").exists());
        assert!(!vault.root.join("state.json").exists());
    }

    #[test]
    fn import_update_keeps_body_filename_and_imported_at() {
        let vault = TempVault::new();
        let dir = vault.root.join("contacts");
        fs::create_dir_all(&dir).unwrap();
        let existing = format!(
            "\
---
source: contacts
id: \"{}\"
title: \"Old Title\"
source_url: \"addressbook://old\"
imported_at: 2020-01-02T03:04:05-07:00
phones:
  - label: \"old\"
    value: \"000\"
---
edited body",
            ada_id()
        );
        assert!(!existing.ends_with('\n'));
        fs::write(dir.join("Custom Name.md"), &existing).unwrap();
        let mut record = load_fixture("ada.json");
        record.display_title = "Renamed".into();
        record.note_body = "\na different note".into();
        let count = import_contacts(
            &dir,
            Ok(vec![record]),
            "2026-09-27T10:00:00-07:00",
            &mut |_, _, _| {},
        )
        .unwrap();
        assert_eq!(count, 1);
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec!["Custom Name.md".to_string()]);
        let text = fs::read_to_string(dir.join("Custom Name.md")).unwrap();
        let (yaml, body) = split_front_matter(&text).unwrap();
        assert_eq!(body, "edited body");
        assert!(text.ends_with("---\nedited body"));
        assert!(!text.ends_with("---\nedited body\n"));
        assert_eq!(
            front_matter_scalar(&yaml, "title").as_deref(),
            Some("Custom Name")
        );
        assert_eq!(
            front_matter_scalar(&yaml, "imported_at").as_deref(),
            Some("2020-01-02T03:04:05-07:00")
        );
        assert!(yaml.contains("+15551212"));
        assert!(!yaml.contains("\nday:"));
        assert!(!yaml.contains("timezone:"));
    }

    #[test]
    fn duplicate_ids_update_utf8_first_path() {
        let vault = TempVault::new();
        let dir = vault.root.join("contacts");
        fs::create_dir_all(&dir).unwrap();
        let id = ada_id();
        let a = format!(
            "\
---
id: \"{id}\"
imported_at: 2020-01-01T00:00:00-07:00
---
body-a"
        );
        let z = format!(
            "\
---
id: \"{id}\"
organization: \"Old\"
imported_at: 1999-01-01T00:00:00-07:00
---
body-z"
        );
        fs::write(dir.join("a.md"), &a).unwrap();
        fs::write(dir.join("z.md"), &z).unwrap();
        let before = fs::read(dir.join("z.md")).unwrap();
        import_contacts(
            &dir,
            Ok(vec![load_fixture("ada.json")]),
            "2026-09-27T10:00:00-07:00",
            &mut |_, _, _| {},
        )
        .unwrap();
        assert_eq!(fs::read(dir.join("z.md")).unwrap(), before);
        let updated = fs::read_to_string(dir.join("a.md")).unwrap();
        assert_ne!(updated, a);
        let (yaml, body) = split_front_matter(&updated).unwrap();
        assert_eq!(body, "body-a");
        assert!(yaml.contains("Analytical Engines"));
        assert!(!fs::read_to_string(dir.join("z.md"))
            .unwrap()
            .contains("Analytical Engines"));
    }

    #[test]
    fn delete_missing_ids_keeps_files_without_id() {
        let vault = TempVault::new();
        let dir = vault.root.join("contacts");
        fs::create_dir_all(&dir).unwrap();
        let keep = "\
---
id: \"keep\"
imported_at: 2020-01-01T00:00:00-07:00
phones:
  - label: \"old\"
    value: \"1\"
---
keep body";
        fs::write(dir.join("Keep.md"), keep).unwrap();
        fs::write(
            dir.join("Other.md"),
            "\
---
id: \"other\"
---
gone
",
        )
        .unwrap();
        let no_id = "\
---
title: \"No Id\"
---
stay
";
        fs::write(dir.join("No Id.md"), no_id).unwrap();
        fs::write(dir.join("notes.txt"), b"text\n").unwrap();
        fs::create_dir_all(dir.join("nested")).unwrap();
        fs::write(dir.join("nested/gone.md"), b"nested\n").unwrap();
        let record = build_record(ContactFields {
            id: "keep".into(),
            display_name: "Kept".into(),
            is_organization: false,
            organization: String::new(),
            job_title: String::new(),
            birthday: None,
            phones: vec![LabeledValue {
                label: "mobile".into(),
                value: "+9".into(),
            }],
            emails: Vec::new(),
            addresses: Vec::new(),
            urls: Vec::new(),
            note: "fresh".into(),
        })
        .unwrap();
        let count = import_contacts(
            &dir,
            Ok(vec![record]),
            "2026-09-27T10:00:00-07:00",
            &mut |_, _, _| {},
        )
        .unwrap();
        assert_eq!(count, 1);
        assert!(!dir.join("Other.md").exists());
        assert_eq!(fs::read(dir.join("No Id.md")).unwrap(), no_id.as_bytes());
        assert_eq!(fs::read(dir.join("notes.txt")).unwrap(), b"text\n");
        assert_eq!(fs::read(dir.join("nested/gone.md")).unwrap(), b"nested\n");
        let updated = fs::read_to_string(dir.join("Keep.md")).unwrap();
        assert_ne!(updated, keep);
        let (yaml, body) = split_front_matter(&updated).unwrap();
        assert_eq!(body, "keep body");
        assert!(yaml.contains("+9"));
    }

    #[test]
    fn company_record_uses_organization_name() {
        let vault = TempVault::new();
        let dir = vault.root.join("contacts");
        let mut labels = Vec::new();
        let count = import_contacts(
            &dir,
            Ok(vec![load_fixture("company.json")]),
            "2026-09-27T10:00:00-07:00",
            &mut |_, _, label| labels.push(label.to_string()),
        )
        .unwrap();
        assert_eq!(count, 1);
        assert!(dir.join("Analytical Engines.md").is_file());
        assert_eq!(labels, vec!["Analytical Engines".to_string()]);
    }

    #[test]
    fn birthday_without_year() {
        let record = load_fixture("birthday-no-year.json");
        assert_eq!(record.extra_front_matter, "birthday: \"--12-10\"\n");
    }

    #[test]
    fn permission_failure_writes_nothing() {
        let absent = TempVault::new();
        let missing = absent.root.join("contacts");
        let err = import_contacts(
            &missing,
            Err(SourceError::new("access denied")),
            "2026-09-27T10:00:00-07:00",
            &mut |_, _, _| {},
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "access denied");
        assert!(!missing.exists());

        let vault = TempVault::new();
        let dir = vault.root.join("contacts");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("Ada.md"), b"original").unwrap();
        fs::create_dir_all(vault.root.join("inbox")).unwrap();
        fs::write(vault.root.join("inbox/old"), b"inbox").unwrap();
        fs::create_dir_all(vault.root.join(".pkmagent")).unwrap();
        fs::write(vault.root.join(".pkmagent/state.json"), b"{}\n").unwrap();
        let err = import_contacts(
            &dir,
            Err(SourceError::new("access denied")),
            "2026-09-27T10:00:00-07:00",
            &mut |_, _, _| {},
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "access denied");
        assert_eq!(fs::read(dir.join("Ada.md")).unwrap(), b"original");
        assert_eq!(fs::read(vault.root.join("inbox/old")).unwrap(), b"inbox");
        assert_eq!(
            fs::read(vault.root.join(".pkmagent/state.json")).unwrap(),
            b"{}\n"
        );
    }

    #[test]
    fn import_error_does_not_create_directory() {
        let vault = TempVault::new();
        let dir = vault.root.join("contacts");
        let err = import_contacts(
            &dir,
            Err(SourceError::new("access denied")),
            "2026-09-27T10:00:00-07:00",
            &mut |_, _, _| {},
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "access denied");
        assert!(!dir.exists());
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn contacts_snapshot_requires_macos() {
        let err = fetch_snapshot().unwrap_err();
        assert_eq!(err.reason, "Contacts is only available on macOS");
        assert!(!Path::new("contacts").exists());
    }
}
