//! Command line. Bad arguments are rejected before the lock is taken.

use std::ffi::OsString;
use std::path::PathBuf;

use chrono::NaiveDate;
use clap::{Parser, Subcommand};

use crate::RunOutput;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Gmail,
    Messages,
    Calendar,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gmail => "gmail",
            Self::Messages => "messages",
            Self::Calendar => "calendar",
        }
    }

    pub const ALL: [Category; 3] = [Self::Gmail, Self::Messages, Self::Calendar];
}

#[derive(Debug)]
pub enum Action {
    AuthGmail,
    Import,
    Collect {
        categories: Vec<Category>,
        since: Option<NaiveDate>,
        day: Option<NaiveDate>,
    },
}

#[derive(Debug)]
pub struct Invocation {
    pub inbox: PathBuf,
    pub action: Action,
}

pub fn parse_args<I, T>(args: I) -> Result<Invocation, RunOutput>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(err) => {
            use clap::error::ErrorKind;
            let text = err.to_string();
            return Err(match err.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => RunOutput {
                    code: 0,
                    stdout: text,
                    stderr: String::new(),
                },
                _ => RunOutput {
                    code: 1,
                    stdout: String::new(),
                    stderr: text,
                },
            });
        }
    };
    let action = match cli.command {
        Commands::Auth {
            target: AuthTarget::Gmail,
        } => Action::AuthGmail,
        Commands::Import { category } => {
            parse_import(&category)?;
            Action::Import
        }
        Commands::Collect {
            category,
            since,
            day,
        } => {
            if since.is_some() && day.is_some() {
                return Err(rejected("--since and --day cannot be combined"));
            }
            let since = since
                .as_deref()
                .map(|value| parse_date(value, "--since"))
                .transpose()?;
            let day = day
                .as_deref()
                .map(|value| parse_date(value, "--day"))
                .transpose()?;
            let categories = parse_collect(&category)?;
            Action::Collect {
                categories,
                since,
                day,
            }
        }
    };
    Ok(Invocation {
        inbox: cli.inbox,
        action,
    })
}

fn parse_import(names: &[String]) -> Result<(), RunOutput> {
    if names.is_empty() {
        return Ok(());
    }
    let mut saw_contacts = false;
    for name in names {
        match name.as_str() {
            "contacts" => {
                if saw_contacts {
                    return Err(rejected("duplicate category: contacts"));
                }
                saw_contacts = true;
            }
            "gmail" | "messages" | "calendar" => {
                return Err(rejected(&format!("import does not allow {name}")));
            }
            other => return Err(rejected(&format!("unknown category: {other}"))),
        }
    }
    Ok(())
}

fn parse_collect(names: &[String]) -> Result<Vec<Category>, RunOutput> {
    if names.is_empty() {
        return Ok(Category::ALL.to_vec());
    }
    let mut selected = Vec::new();
    for name in names {
        let category = match name.as_str() {
            "gmail" => Category::Gmail,
            "messages" => Category::Messages,
            "calendar" => Category::Calendar,
            "contacts" => return Err(rejected("collect does not allow contacts")),
            other => return Err(rejected(&format!("unknown category: {other}"))),
        };
        if selected.contains(&category) {
            return Err(rejected(&format!("duplicate category: {name}")));
        }
        selected.push(category);
    }
    selected.sort();
    Ok(selected)
}

fn parse_date(value: &str, flag: &str) -> Result<NaiveDate, RunOutput> {
    if value.len() != 10
        || value.as_bytes().get(4) != Some(&b'-')
        || value.as_bytes().get(7) != Some(&b'-')
    {
        return Err(rejected(&format!("{flag} is not a date (YYYY-MM-DD)")));
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| rejected(&format!("{flag} is not a date (YYYY-MM-DD)")))
}

pub fn rejected(message: &str) -> RunOutput {
    RunOutput {
        code: 1,
        stdout: String::new(),
        stderr: with_newline(message),
    }
}

pub fn with_newline(message: &str) -> String {
    if message.ends_with('\n') {
        message.to_string()
    } else {
        format!("{message}\n")
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "pkmagent",
    about = "Import Gmail, Messages, Calendar, and Contacts into a vault"
)]
struct Cli {
    /// Inbox directory. The vault is its parent.
    #[arg(long, global = true, value_name = "PATH", default_value = "./inbox")]
    inbox: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Authorize an account.
    Auth {
        #[command(subcommand)]
        target: AuthTarget,
    },
    /// Update contacts in the vault.
    Import {
        /// Only `contacts` is allowed. Omit it to import contacts.
        category: Vec<String>,
    },
    /// Collect Gmail, Messages, and Calendar into a new inbox folder.
    Collect {
        /// Categories: gmail, messages, calendar.
        category: Vec<String>,
        /// Local start date for categories that have no cursor.
        #[arg(long, value_name = "YYYY-MM-DD")]
        since: Option<String>,
        /// Collect one civil day and do not move cursors.
        #[arg(long, value_name = "YYYY-MM-DD")]
        day: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum AuthTarget {
    /// Store a Gmail refresh token.
    Gmail,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Invocation, RunOutput> {
        let mut full = vec!["pkmagent"];
        full.extend_from_slice(args);
        parse_args(full)
    }

    #[test]
    fn rejects_categories_the_command_does_not_allow() {
        for args in [
            &["import", "gmail"][..],
            &["import", "messages"][..],
            &["import", "calendar"][..],
            &["collect", "contacts"][..],
        ] {
            let err = parse(args).unwrap_err();
            assert_eq!(err.code, 1, "{args:?} {}", err.stderr);
        }
        assert!(parse(&["import", "gmail"])
            .unwrap_err()
            .stderr
            .contains("import does not allow gmail"));
        assert!(parse(&["collect", "contacts"])
            .unwrap_err()
            .stderr
            .contains("collect does not allow contacts"));
        assert!(parse(&["collect", "nope"])
            .unwrap_err()
            .stderr
            .contains("unknown category"));
        assert!(parse(&["import", "contacts", "contacts"])
            .unwrap_err()
            .stderr
            .contains("duplicate category"));
        assert!(parse(&["collect", "gmail", "gmail"])
            .unwrap_err()
            .stderr
            .contains("duplicate category"));
        assert_eq!(
            parse(&["collect", "--since", "2026-09-26", "--day", "2026-09-26"])
                .unwrap_err()
                .code,
            1
        );
        assert!(parse(&["collect", "--since", "2026-13-40"])
            .unwrap_err()
            .stderr
            .contains("not a date"));
        assert!(parse(&["import", "--since", "2026-09-26"]).is_err());
        assert!(parse(&["import", "--day", "2026-09-26"]).is_err());
    }

    #[test]
    fn typed_order_is_ignored_and_defaults_cover_every_allowed_category() {
        let collect = parse(&["collect", "calendar", "gmail"]).unwrap();
        match collect.action {
            Action::Collect { categories, .. } => {
                assert_eq!(categories, vec![Category::Gmail, Category::Calendar]);
            }
            other => panic!("unexpected {other:?}"),
        }
        let all = parse(&["collect"]).unwrap();
        match all.action {
            Action::Collect {
                categories,
                since,
                day,
            } => {
                assert_eq!(categories, Category::ALL.to_vec());
                assert!(since.is_none() && day.is_none());
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(parse(&["import"]).unwrap().action, Action::Import));
        assert!(matches!(
            parse(&["import", "contacts"]).unwrap().action,
            Action::Import
        ));
        assert!(matches!(
            parse(&["auth", "gmail"]).unwrap().action,
            Action::AuthGmail
        ));
        assert_eq!(parse(&["--help"]).unwrap_err().code, 0);
        assert_eq!(parse(&["import"]).unwrap().inbox, PathBuf::from("./inbox"));
        assert_eq!(
            parse(&["--inbox", "vault/inbox", "collect"]).unwrap().inbox,
            PathBuf::from("vault/inbox")
        );
    }
}
