//! Vault paths, the exclusive lock, and collect cursors.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, FixedOffset};

use crate::cli::Category;
use crate::timeutil::{self, format_rfc3339};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursors {
    pub gmail: Option<DateTime<FixedOffset>>,
    pub messages: Option<DateTime<FixedOffset>>,
    pub calendar: Option<DateTime<FixedOffset>>,
}

impl Default for Cursors {
    fn default() -> Self {
        Self {
            gmail: None,
            messages: None,
            calendar: None,
        }
    }
}

impl Cursors {
    pub fn get(&self, category: Category) -> Option<DateTime<FixedOffset>> {
        match category {
            Category::Gmail => self.gmail,
            Category::Messages => self.messages,
            Category::Calendar => self.calendar,
        }
    }

    pub fn set(&mut self, category: Category, value: DateTime<FixedOffset>) {
        match category {
            Category::Gmail => self.gmail = Some(value),
            Category::Messages => self.messages = Some(value),
            Category::Calendar => self.calendar = Some(value),
        }
    }
}

#[derive(Debug)]
pub enum StateError {
    Corrupt,
    Unreadable(io::Error),
}

#[derive(Debug)]
pub enum LockError {
    Exists,
    Io(io::Error),
}

pub struct Lock {
    path: PathBuf,
}

impl Lock {
    pub fn acquire(vault: &Path) -> Result<Self, LockError> {
        let dir = vault.join(".pkmagent");
        fs::create_dir_all(&dir).map_err(LockError::Io)?;
        let path = dir.join("pkmagent.lock");
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                writeln!(file, "{}", std::process::id()).map_err(LockError::Io)?;
                Ok(Self { path })
            }
            Err(err) => {
                if path.exists() {
                    Err(LockError::Exists)
                } else {
                    Err(LockError::Io(err))
                }
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn vault_root(inbox: &Path) -> PathBuf {
    match inbox.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

pub fn secrets_dir(vault: &Path) -> PathBuf {
    vault.join(".pkmagent").join("secrets")
}

pub fn gmail_client_path(vault: &Path) -> PathBuf {
    secrets_dir(vault).join("gmail-client.json")
}

pub fn state_path(vault: &Path) -> PathBuf {
    vault.join(".pkmagent").join("state.json")
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn lock_path(vault: &Path) -> PathBuf {
    vault.join(".pkmagent").join("pkmagent.lock")
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn ensure_secrets_dir(vault: &Path) -> io::Result<PathBuf> {
    let parent = vault.join(".pkmagent");
    fs::create_dir_all(&parent)?;
    let dir = parent.join("secrets");
    if !dir.exists() {
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&dir)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir)
}

pub fn load_cursors(vault: &Path) -> Result<Cursors, StateError> {
    let path = state_path(vault);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Cursors::default()),
        Err(err) => return Err(StateError::Unreadable(err)),
    };
    parse_cursors(&text).ok_or(StateError::Corrupt)
}

pub fn store_cursors(vault: &Path, cursors: &Cursors) -> io::Result<()> {
    let dir = vault.join(".pkmagent");
    fs::create_dir_all(&dir)?;
    atomic_write(
        &dir.join("state.json"),
        render_cursors(cursors).as_bytes(),
        None,
    )
}

pub fn parse_cursors(text: &str) -> Option<Cursors> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;
    for key in object.keys() {
        if key != "gmail" && key != "messages" && key != "calendar" {
            return None;
        }
    }
    Some(Cursors {
        gmail: read_cursor(object.get("gmail"))?,
        messages: read_cursor(object.get("messages"))?,
        calendar: read_cursor(object.get("calendar"))?,
    })
}

fn read_cursor(value: Option<&serde_json::Value>) -> Option<Option<DateTime<FixedOffset>>> {
    match value {
        None | Some(serde_json::Value::Null) => Some(None),
        Some(serde_json::Value::String(text)) => timeutil::parse_offset_rfc3339(text).map(Some),
        _ => None,
    }
}

pub fn render_cursors(cursors: &Cursors) -> String {
    fn field(name: &str, value: Option<DateTime<FixedOffset>>) -> String {
        match value {
            None => format!("  \"{name}\": null"),
            Some(dt) => format!("  \"{name}\": \"{}\"", format_rfc3339(dt)),
        }
    }
    format!(
        "{{\n{},\n{},\n{}\n}}\n",
        field("gmail", cursors.gmail),
        field("messages", cursors.messages),
        field("calendar", cursors.calendar)
    )
}

pub fn atomic_write(dest: &Path, bytes: &[u8], mode: Option<u32>) -> io::Result<()> {
    let parent = dest.parent().unwrap_or_else(|| Path::new("."));
    let file_name = dest.file_name().unwrap_or_default().to_string_lossy();
    let tmp = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    let write_result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok::<(), io::Error>(())
    })();
    if let Err(err) = write_result {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    if let Err(err) = fs::rename(&tmp, dest) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

pub fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    text.strip_prefix("./").unwrap_or(&text).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempVault;
    use chrono::DateTime;

    #[test]
    fn missing_state_means_unset_cursors() {
        let vault = TempVault::new();
        let cursors = load_cursors(&vault.root).unwrap();
        assert_eq!(cursors, Cursors::default());
    }

    #[test]
    fn cursors_round_trip_with_gmail_first() {
        let vault = TempVault::new();
        let mut cursors = Cursors::default();
        cursors.gmail = Some(DateTime::parse_from_rfc3339("2026-09-27T18:04:11-07:00").unwrap());
        store_cursors(&vault.root, &cursors).unwrap();
        let text = fs::read_to_string(state_path(&vault.root)).unwrap();
        assert!(text.starts_with("{\n  \"gmail\":"));
        assert!(text.contains("\"messages\": null"));
        assert_eq!(load_cursors(&vault.root).unwrap(), cursors);
        assert!(!vault.root.join(".pkmagent").join(".state.json").exists());
    }

    #[test]
    fn corrupt_state_shapes_are_rejected() {
        for text in [
            r#"{"gmail":null,"messages":null,"calendar":null,"extra":1}"#,
            r#"{"gmail":"2026-09-27T18:04:11Z","messages":null,"calendar":null}"#,
            r#"{"gmail":"2026-09-27T18:04:11.5-07:00","messages":null,"calendar":null}"#,
            r#"["gmail"]"#,
            r#"{"gmail":1,"messages":null,"calendar":null}"#,
        ] {
            assert!(parse_cursors(text).is_none(), "{text}");
        }
    }

    #[test]
    fn atomic_write_replaces_and_sets_mode() {
        let vault = TempVault::new();
        let dest = vault.root.join("secret.json");
        atomic_write(&dest, b"one\n", Some(0o600)).unwrap();
        atomic_write(&dest, b"two\n", Some(0o600)).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"two\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let secrets = ensure_secrets_dir(&vault.root).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&secrets).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
    }

    #[test]
    fn lock_records_the_pid_and_is_removed_on_drop() {
        let vault = TempVault::new();
        let path = lock_path(&vault.root);
        let text = {
            let lock = Lock::acquire(&vault.root).unwrap();
            assert!(path.is_file());
            let text = fs::read_to_string(&path).unwrap();
            assert!(Lock::acquire(&vault.root).is_err());
            drop(lock);
            text
        };
        assert!(text.contains(&std::process::id().to_string()));
        assert!(!path.exists());
    }

    #[test]
    fn display_path_strips_a_dot_slash_prefix() {
        assert_eq!(
            display_path(Path::new("./inbox/2026-09-27T100000-0700")),
            "inbox/2026-09-27T100000-0700"
        );
    }
}
