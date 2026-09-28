//! `pkmagent` maps a typed outcome to the process status. Library code does not exit.

mod cli;
mod collect;
mod contacts_index;
mod filename;
mod note;
mod progress;
mod sources;
mod timeutil;
mod vault;

#[cfg(test)]
mod testutil;

use std::io::{self, Write};
use std::path::Path;

use crate::cli::{parse_args, Action};
use crate::collect::{run_collect, run_import, CollectParams, Runtime};
use crate::progress::SharedSink;
use crate::sources::{
    calendar::CalendarSource, gmail, gmail::GmailSource, messages::MessagesSource, CollectSource,
};
use crate::timeutil::Clock;

#[derive(Debug)]
pub struct RunOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

fn main() {
    let sink = SharedSink::stderr();
    let runtime = Runtime::production(Clock::local());
    let output = execute(std::env::args_os(), &runtime, &sink);
    if !output.stdout.is_empty() {
        print!("{}", output.stdout);
        let _ = io::stdout().flush();
    }
    if !output.stderr.is_empty() {
        eprint!("{}", output.stderr);
    }
    if output.code != 0 {
        std::process::exit(output.code);
    }
}

pub fn execute<I, T>(args: I, runtime: &Runtime, sink: &SharedSink) -> RunOutput
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let invocation = match parse_args(args) {
        Ok(invocation) => invocation,
        Err(output) => return output,
    };
    let vault = vault::vault_root(&invocation.inbox);
    match invocation.action {
        Action::AuthGmail => authorize(&vault, sink),
        Action::Import => run_import(&vault, &runtime.clock, sink),
        Action::Collect {
            categories,
            since,
            day,
        } => {
            let zone = runtime.clock.zone;
            let secrets = vault::secrets_dir(&vault);
            let default_gmail;
            let default_messages;
            let default_calendar;
            let gmail: &dyn CollectSource = if let Some(source) = &runtime.gmail {
                source.as_ref()
            } else {
                default_gmail = GmailSource {
                    secrets_dir: secrets,
                    zone,
                };
                &default_gmail
            };
            let messages: &dyn CollectSource = if let Some(source) = &runtime.messages {
                source.as_ref()
            } else {
                default_messages = MessagesSource { zone };
                &default_messages
            };
            let calendar: &dyn CollectSource = if let Some(source) = &runtime.calendar {
                source.as_ref()
            } else {
                default_calendar = CalendarSource { zone };
                &default_calendar
            };
            run_collect(CollectParams {
                vault: &vault,
                inbox: &invocation.inbox,
                categories: &categories,
                since,
                day,
                clock: &runtime.clock,
                sink,
                gmail,
                messages,
                calendar,
                write_file: runtime.write_file.as_ref(),
            })
        }
    }
}

fn authorize(vault: &Path, sink: &SharedSink) -> RunOutput {
    if !vault::gmail_client_path(vault).is_file() {
        sink.write_stderr("gmail-client.json is missing\n");
        return captured(1, String::new(), sink);
    }
    match gmail::authorize(&vault::secrets_dir(vault)) {
        Ok(()) => captured(0, String::new(), sink),
        Err(err) => {
            sink.write_stderr(&format!("{err}\n"));
            captured(1, String::new(), sink)
        }
    }
}

fn captured(code: i32, stdout: String, sink: &SharedSink) -> RunOutput {
    RunOutput {
        code,
        stdout,
        stderr: sink.captured(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempVault;
    use std::fs;

    fn runtime() -> Runtime {
        Runtime::production(Clock {
            zone: chrono_tz::America::Los_Angeles,
            now: chrono::DateTime::parse_from_rfc3339("2026-09-27T10:00:00-07:00").unwrap(),
        })
    }

    #[test]
    fn bad_arguments_do_not_create_pkmagent() {
        let vault = TempVault::new();
        let inbox = vault.inbox();
        let sink = SharedSink::buffer(false);
        let rt = runtime();
        for args in [
            vec![
                "pkmagent",
                "--inbox",
                inbox.to_str().unwrap(),
                "import",
                "gmail",
            ],
            vec![
                "pkmagent",
                "--inbox",
                inbox.to_str().unwrap(),
                "import",
                "messages",
            ],
            vec![
                "pkmagent",
                "--inbox",
                inbox.to_str().unwrap(),
                "import",
                "calendar",
            ],
            vec![
                "pkmagent",
                "--inbox",
                inbox.to_str().unwrap(),
                "collect",
                "contacts",
            ],
            vec![
                "pkmagent",
                "--inbox",
                inbox.to_str().unwrap(),
                "collect",
                "gmail",
                "gmail",
            ],
            vec![
                "pkmagent",
                "--inbox",
                inbox.to_str().unwrap(),
                "collect",
                "nope",
            ],
            vec![
                "pkmagent",
                "--inbox",
                inbox.to_str().unwrap(),
                "collect",
                "--since",
                "2026-09-26",
                "--day",
                "2026-09-26",
            ],
        ] {
            let output = execute(args.clone(), &rt, &sink);
            assert_eq!(output.code, 1, "{args:?} {}", output.stderr);
            assert!(!vault.root.join(".pkmagent").exists(), "{args:?}");
            assert!(!inbox.exists(), "{args:?}");
        }
    }

    #[test]
    fn auth_checks_the_client_file_before_any_lock() {
        let vault = TempVault::new();
        let inbox = vault.inbox();
        let sink = SharedSink::buffer(false);
        let rt = runtime();
        let missing = execute(
            [
                "pkmagent",
                "--inbox",
                inbox.to_str().unwrap(),
                "auth",
                "gmail",
            ],
            &rt,
            &sink,
        );
        assert_eq!(missing.code, 1);
        assert!(missing.stderr.contains("gmail-client.json is missing"));
        assert!(!vault.root.join(".pkmagent").exists());

        let secrets = vault.root.join(".pkmagent").join("secrets");
        fs::create_dir_all(&secrets).unwrap();
        fs::write(secrets.join("gmail-client.json"), "{}").unwrap();
        let sink = SharedSink::buffer(false);
        let present = execute(
            [
                "pkmagent",
                "--inbox",
                inbox.to_str().unwrap(),
                "auth",
                "gmail",
            ],
            &rt,
            &sink,
        );
        assert_eq!(present.code, 1);
        assert!(present
            .stderr
            .contains("Gmail authorization is not implemented"));
        assert!(!vault::lock_path(&vault.root).exists());
    }

    #[test]
    fn info_plist_has_the_usage_descriptions() {
        let text = include_str!("../assets/Info.plist");
        assert!(text.contains("NSCalendarsFullAccessUsageDescription"));
        assert!(text.contains("NSCalendarsUsageDescription"));
        assert!(text.contains("NSContactsUsageDescription"));
        assert!(text.contains("pkmagent reads your calendars to copy events into the inbox."));
        assert!(text.contains(
            "pkmagent reads your contacts to copy them into the vault and to link them from collected notes."
        ));
    }
}
