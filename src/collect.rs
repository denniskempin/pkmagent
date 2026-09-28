//! Collect orchestration and the contacts import path.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, FixedOffset, NaiveDate};
use chrono_tz::Tz;

use crate::cli::{with_newline, Category};
use crate::contacts_index::ContactIndex;
use crate::filename::allocate_filename;
use crate::note::{render_note, NoteHeader};
use crate::progress::{format_line, SharedSink};
use crate::sources::contacts;
use crate::sources::{CollectSource, CollectedItem};
use crate::timeutil::{
    self, civil_dates, floor_second, format_rfc3339, local_midnight, slice_bounds, whole_day_slice,
    Clock, Slice,
};
use crate::vault::{self, Cursors, Lock, LockError, StateError};
use crate::RunOutput;

struct CategoryWindow {
    category: Category,
    start: DateTime<FixedOffset>,
}

struct RunPlan {
    end: DateTime<FixedOffset>,
    zone: Tz,
    day_mode: bool,
    dates: Vec<NaiveDate>,
    categories: Vec<CategoryWindow>,
}

pub struct Runtime {
    pub clock: Clock,
    pub gmail: Option<Arc<dyn CollectSource>>,
    pub messages: Option<Arc<dyn CollectSource>>,
    pub calendar: Option<Arc<dyn CollectSource>>,
    pub write_file: Arc<dyn Fn(&Path, &[u8]) -> io::Result<()> + Send + Sync>,
}

impl Runtime {
    pub fn production(clock: Clock) -> Self {
        Self {
            clock,
            gmail: None,
            messages: None,
            calendar: None,
            write_file: Arc::new(|path, bytes| fs::write(path, bytes)),
        }
    }
}

struct JobError {
    day: NaiveDate,
    category: Category,
    reason: String,
}

struct Fetched {
    category: Category,
    items: Vec<CollectedItem>,
}

struct Bucket {
    category: Category,
    dir_date: Option<NaiveDate>,
    relative_dir: String,
    items: Vec<CollectedItem>,
}

struct ProgressState {
    order: Vec<Category>,
    total: u64,
    done: BTreeMap<Category, u64>,
    label: BTreeMap<Category, String>,
}

pub fn run_import(vault: &Path, clock: &Clock, sink: &SharedSink) -> RunOutput {
    let _lock = match take_lock(vault, sink) {
        Ok(lock) => lock,
        Err(output) => return output,
    };
    let snapshot = contacts::fetch_snapshot();
    if let Ok(people) = &snapshot {
        sink.write_progress(&[format_line("contacts", 0, people.len() as u64, "waiting")]);
    }
    let imported_at = format_rfc3339(clock.now);
    let mut progress = |done, total, label: &str| {
        sink.write_progress(&[format_line("contacts", done, total, label)]);
    };
    match contacts::import_contacts(
        &vault.join("contacts"),
        snapshot,
        &imported_at,
        &mut progress,
    ) {
        Ok(count) => finish(0, format!("contacts={count}\n"), sink),
        Err(err) => {
            sink.write_stderr(&format!("contacts failed: {err}\n"));
            finish(2, String::new(), sink)
        }
    }
}

pub struct CollectParams<'a> {
    pub vault: &'a Path,
    pub inbox: &'a Path,
    pub categories: &'a [Category],
    pub since: Option<NaiveDate>,
    pub day: Option<NaiveDate>,
    pub clock: &'a Clock,
    pub sink: &'a SharedSink,
    pub gmail: &'a dyn CollectSource,
    pub messages: &'a dyn CollectSource,
    pub calendar: &'a dyn CollectSource,
    pub write_file: &'a (dyn Fn(&Path, &[u8]) -> io::Result<()> + Sync),
}

pub fn run_collect(params: CollectParams<'_>) -> RunOutput {
    let mut categories = params.categories.to_vec();
    categories.sort();
    let categories = categories.as_slice();
    let _lock = match take_lock(params.vault, params.sink) {
        Ok(lock) => lock,
        Err(output) => return output,
    };

    let cursors = if params.day.is_some() {
        Cursors::default()
    } else {
        match vault::load_cursors(params.vault) {
            Ok(cursors) => cursors,
            Err(StateError::Corrupt) => {
                params.sink.write_stderr("state.json is corrupt\n");
                return finish(1, String::new(), params.sink);
            }
            Err(StateError::Unreadable(err)) => {
                params
                    .sink
                    .write_stderr(&format!("state.json is unreadable: {err}\n"));
                return finish(1, String::new(), params.sink);
            }
        }
    };

    let plan = match plan_collect(
        params.clock.zone,
        params.clock.now,
        categories,
        &cursors,
        params.since,
        params.day,
    ) {
        Ok(plan) => plan,
        Err(message) => {
            params.sink.write_stderr(&with_newline(&message));
            return finish(1, String::new(), params.sink);
        }
    };

    let index = ContactIndex::load(&params.vault.join("contacts"));
    let fetched = match fetch_all(&params, &plan, &index) {
        Ok(fetched) => fetched,
        Err(err) => {
            params.sink.write_stderr(&format!(
                "{} failed {}: {}\n",
                err.day,
                err.category.as_str(),
                err.reason
            ));
            return finish(2, String::new(), params.sink);
        }
    };

    let dated = plan.dates.len() > 2;
    let mut groups: BTreeMap<(String, String), Bucket> = BTreeMap::new();
    for batch in fetched {
        for item in batch.items {
            let relative_dir = if dated {
                format!("{}/{}", item.file_day, batch.category.as_str())
            } else {
                batch.category.as_str().to_string()
            };
            let key = (relative_dir.clone(), item.stable_id.clone());
            groups
                .entry(key)
                .or_insert_with(|| Bucket {
                    category: batch.category,
                    dir_date: dated.then_some(item.file_day),
                    relative_dir,
                    items: Vec::new(),
                })
                .items
                .push(item);
        }
    }

    if groups.is_empty() {
        if let Err(message) = move_cursors(params.vault, &plan, categories) {
            params.sink.write_stderr(&message);
            return finish(2, String::new(), params.sink);
        }
        return finish(0, render_stdout(None, categories, dated, &[]), params.sink);
    }

    let run_dir = pick_run_dir(params.inbox, &timeutil::run_folder_name(plan.end));
    let mut taken_dir = String::new();
    let mut taken = Vec::new();
    let mut written = Vec::new();
    for ((_, stable_id), bucket) in &mut groups {
        if bucket.relative_dir != taken_dir {
            taken.clear();
            taken_dir = bucket.relative_dir.clone();
        }
        bucket.items.sort_by(|left, right| {
            left.t
                .cmp(&right.t)
                .then_with(|| left.tie_break.cmp(&right.tie_break))
        });
        let filename = allocate_filename(
            &bucket.items.last().expect("group").display_title,
            &bucket.items.last().expect("group").empty_title_fallback,
            stable_id,
            &taken,
        );
        taken.push(filename.clone());
        let text = render_group(&plan, &params, bucket, stable_id, &filename);
        let path = run_dir.join(&bucket.relative_dir).join(&filename);
        if let Some(parent) = path.parent() {
            if let Err(err) = fs::create_dir_all(parent) {
                return write_failed(params.sink, &plan, bucket, &err);
            }
        }
        if let Err(err) = (params.write_file)(&path, text.as_bytes()) {
            return write_failed(params.sink, &plan, bucket, &err);
        }
        written.push((bucket.category, bucket.dir_date));
    }

    if let Err(message) = move_cursors(params.vault, &plan, categories) {
        params.sink.write_stderr(&message);
        return finish(2, String::new(), params.sink);
    }
    let folder = vault::display_path(&run_dir);
    finish(
        0,
        render_stdout(Some(&folder), categories, dated, &written),
        params.sink,
    )
}

fn write_failed(sink: &SharedSink, plan: &RunPlan, bucket: &Bucket, err: &io::Error) -> RunOutput {
    let day = bucket
        .dir_date
        .or_else(|| bucket.items.first().map(|item| item.file_day))
        .or_else(|| plan.dates.first().copied())
        .unwrap_or_else(|| plan.end.date_naive());
    sink.write_stderr(&format!(
        "{day} failed {}: write failed: {err}\n",
        bucket.category.as_str()
    ));
    finish(2, String::new(), sink)
}

fn render_group(
    plan: &RunPlan,
    params: &CollectParams<'_>,
    bucket: &Bucket,
    stable_id: &str,
    filename: &str,
) -> String {
    let source = source_for(params, bucket.category);
    let first = bucket.items.first().expect("group");
    let title = filename.strip_suffix(".md").unwrap_or(filename).to_string();
    let mut attachments = Vec::new();
    for item in &bucket.items {
        attachments.extend(item.attachments.iter().cloned());
    }
    let header = NoteHeader {
        source: bucket.category.as_str().to_string(),
        id: stable_id.to_string(),
        title,
        source_url: first.source_url.clone(),
        imported_at: format_rfc3339(plan.end),
        day: Some(first.file_day.to_string()),
        timezone: Some(plan.zone.name().to_string()),
        extra_front_matter: source.group_extra_front_matter(&bucket.items),
    };
    render_note(&header, &join_bodies(&bucket.items), &attachments)
}

fn join_bodies(items: &[CollectedItem]) -> String {
    let mut body = String::from("\n");
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            body.push('\n');
        }
        body.push_str(&item.body);
    }
    body
}

fn move_cursors(vault: &Path, plan: &RunPlan, selected: &[Category]) -> Result<(), String> {
    if plan.day_mode {
        return Ok(());
    }
    let mut cursors =
        vault::load_cursors(vault).map_err(|_| "writing state.json failed".to_string())?;
    for category in selected {
        cursors.set(*category, plan.end);
    }
    let day = plan
        .dates
        .first()
        .copied()
        .unwrap_or_else(|| plan.end.date_naive());
    let category = selected.first().copied().unwrap_or(Category::Gmail);
    vault::store_cursors(vault, &cursors).map_err(|_| {
        format!(
            "{day} failed {}: writing state.json failed\n",
            category.as_str()
        )
    })
}

fn fetch_all(
    params: &CollectParams<'_>,
    plan: &RunPlan,
    index: &ContactIndex,
) -> Result<Vec<Fetched>, JobError> {
    let total = plan.dates.len() as u64;
    let progress = Mutex::new(ProgressState {
        order: plan
            .categories
            .iter()
            .map(|window| window.category)
            .collect(),
        total,
        done: plan
            .categories
            .iter()
            .map(|window| (window.category, 0))
            .collect(),
        label: plan
            .categories
            .iter()
            .map(|window| (window.category, "waiting".to_string()))
            .collect(),
    });
    redraw(params.sink, &progress);
    let cancel = AtomicBool::new(false);
    let cancel = &cancel;
    let progress = &progress;
    let mut errors = Vec::new();
    let mut fetched = Vec::new();
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for window in &plan.categories {
            let category = window.category;
            let start = window.start;
            let source = source_for(params, category);
            let cancel = cancel;
            let progress = progress;
            let plan = plan;
            let index = index;
            let sink = params.sink;
            handles.push(scope.spawn(move || {
                fetch_category(category, start, source, plan, index, cancel, progress, sink)
            }));
        }
        for (window, handle) in plan.categories.iter().zip(handles) {
            match handle.join() {
                Ok(Ok(part)) => fetched.extend(part),
                Ok(Err(err)) => errors.push(err),
                Err(_) => errors.push(JobError {
                    day: plan
                        .dates
                        .first()
                        .copied()
                        .unwrap_or_else(|| plan.end.date_naive()),
                    category: window.category,
                    reason: "fetch panicked".to_string(),
                }),
            }
        }
    });
    if let Some(err) = errors.into_iter().next() {
        Err(err)
    } else {
        Ok(fetched)
    }
}

fn fetch_category(
    category: Category,
    start: DateTime<FixedOffset>,
    source: &dyn CollectSource,
    plan: &RunPlan,
    index: &ContactIndex,
    cancel: &AtomicBool,
    progress: &Mutex<ProgressState>,
    sink: &SharedSink,
) -> Result<Vec<Fetched>, JobError> {
    let mut fetched = Vec::new();
    for day in &plan.dates {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let slice = if plan.day_mode {
            Some(whole_day_slice(*day, plan.zone))
        } else {
            slice_bounds(start, plan.end, *day, plan.zone)
        };
        let Some(slice) = slice else {
            mark(progress, sink, category, *day, true);
            continue;
        };
        mark(progress, sink, category, slice.day, false);
        match source.fetch(&slice, index) {
            Ok(items) => {
                let items = keep_in_slice(items, &slice);
                fetched.push(Fetched { category, items });
                mark(progress, sink, category, slice.day, true);
            }
            Err(err) => {
                cancel.store(true, Ordering::SeqCst);
                return Err(JobError {
                    day: slice.day,
                    category,
                    reason: err.to_string(),
                });
            }
        }
    }
    Ok(fetched)
}

fn keep_in_slice(mut items: Vec<CollectedItem>, slice: &Slice) -> Vec<CollectedItem> {
    items.retain(|item| item.t >= slice.start && item.t < slice.end);
    items
}

fn mark(
    progress: &Mutex<ProgressState>,
    sink: &SharedSink,
    category: Category,
    day: NaiveDate,
    finished: bool,
) {
    let mut state = progress.lock().expect("progress");
    state.label.insert(category, day.to_string());
    if finished {
        *state.done.entry(category).or_default() += 1;
    }
    drop(state);
    redraw(sink, progress);
}

fn redraw(sink: &SharedSink, progress: &Mutex<ProgressState>) {
    let state = progress.lock().expect("progress");
    let lines = state
        .order
        .iter()
        .map(|category| {
            format_line(
                category.as_str(),
                state.done.get(category).copied().unwrap_or(0),
                state.total,
                state
                    .label
                    .get(category)
                    .map(String::as_str)
                    .unwrap_or("waiting"),
            )
        })
        .collect::<Vec<_>>();
    drop(state);
    sink.write_progress(&lines);
}

fn source_for<'a>(params: &'a CollectParams<'a>, category: Category) -> &'a dyn CollectSource {
    match category {
        Category::Gmail => params.gmail,
        Category::Messages => params.messages,
        Category::Calendar => params.calendar,
    }
}

fn plan_collect(
    zone: Tz,
    end: DateTime<FixedOffset>,
    selected: &[Category],
    cursors: &Cursors,
    since: Option<NaiveDate>,
    day: Option<NaiveDate>,
) -> Result<RunPlan, String> {
    let end = floor_second(end);
    if let Some(day) = day {
        let start = local_midnight(day, zone);
        return Ok(RunPlan {
            end,
            zone,
            day_mode: true,
            dates: vec![day],
            categories: selected
                .iter()
                .copied()
                .map(|category| CategoryWindow { category, start })
                .collect(),
        });
    }
    if let Some(since_day) = since {
        if local_midnight(since_day, zone) > end {
            return Err("--since is after the run ends".to_string());
        }
    }
    let mut categories = Vec::with_capacity(selected.len());
    let mut missing_cursor = false;
    let mut has_cursor = false;
    for category in selected {
        match cursors.get(*category) {
            Some(cursor) => {
                has_cursor = true;
                if end < cursor {
                    return Err(format!(
                        "frozen end is earlier than the {} cursor",
                        category.as_str()
                    ));
                }
                categories.push(CategoryWindow {
                    category: *category,
                    start: cursor,
                });
            }
            None => {
                missing_cursor = true;
                let since_day = since.ok_or_else(|| "--since is required".to_string())?;
                categories.push(CategoryWindow {
                    category: *category,
                    start: local_midnight(since_day, zone),
                });
            }
        }
    }
    if since.is_some() && has_cursor && !missing_cursor {
        return Err("--since is not allowed when every selected category has a cursor".to_string());
    }
    let earliest = categories
        .iter()
        .map(|window| window.start)
        .min()
        .unwrap_or(end);
    Ok(RunPlan {
        end,
        zone,
        day_mode: false,
        dates: civil_dates(earliest, end, zone),
        categories,
    })
}

fn pick_run_dir(inbox: &Path, stamp: &str) -> PathBuf {
    let primary = inbox.join(stamp);
    if !primary.exists() {
        return primary;
    }
    let mut n = 2u32;
    loop {
        let candidate = inbox.join(format!("{stamp}-{n}"));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

fn render_stdout(
    folder: Option<&str>,
    selected: &[Category],
    dated: bool,
    written: &[(Category, Option<NaiveDate>)],
) -> String {
    let mut out = String::new();
    if let Some(folder) = folder {
        out.push_str(folder);
        out.push('\n');
    }
    let any = !written.is_empty();
    for category in selected {
        let mine: Vec<_> = written
            .iter()
            .filter(|(cat, _)| *cat == *category)
            .collect();
        if !any || mine.is_empty() {
            out.push_str(&format!("{}=0\n", category.as_str()));
            continue;
        }
        if dated {
            let mut by_date: BTreeMap<NaiveDate, usize> = BTreeMap::new();
            for (_, day) in mine {
                if let Some(day) = day {
                    *by_date.entry(*day).or_default() += 1;
                }
            }
            for (day, count) in by_date {
                out.push_str(&format!("{day} {}={count}\n", category.as_str()));
            }
        } else {
            out.push_str(&format!("{}={}\n", category.as_str(), mine.len()));
        }
    }
    out
}

fn take_lock(vault: &Path, sink: &SharedSink) -> Result<Lock, RunOutput> {
    match Lock::acquire(vault) {
        Ok(lock) => Ok(lock),
        Err(LockError::Exists) => {
            sink.write_stderr("pkmagent.lock exists; delete it when nothing is running\n");
            Err(finish(1, String::new(), sink))
        }
        Err(LockError::Io(err)) => {
            sink.write_stderr(&format!("pkmagent.lock: {err}\n"));
            Err(finish(1, String::new(), sink))
        }
    }
}

fn finish(code: i32, stdout: String, sink: &SharedSink) -> RunOutput {
    RunOutput {
        code,
        stdout,
        stderr: sink.captured(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::SharedSink;
    use crate::sources::{test_item, MapSource};
    use crate::testutil::TempVault;
    use chrono::DateTime;
    use chrono_tz::America::Los_Angeles;
    use std::fs;
    use std::io;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn clock(stamp: &str) -> Clock {
        Clock {
            zone: Los_Angeles,
            now: DateTime::parse_from_rfc3339(stamp).unwrap(),
        }
    }

    fn at(stamp: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(stamp).unwrap()
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    struct Sources {
        gmail: MapSource,
        messages: MapSource,
        calendar: MapSource,
    }

    fn sources(
        gmail: Vec<CollectedItem>,
        messages: Vec<CollectedItem>,
        calendar: Vec<CollectedItem>,
    ) -> Sources {
        Sources {
            gmail: MapSource::new(gmail),
            messages: MapSource::new(messages),
            calendar: MapSource::new(calendar),
        }
    }

    fn exercise(
        vault: &TempVault,
        clock: Clock,
        categories: &[Category],
        since: Option<NaiveDate>,
        day: Option<NaiveDate>,
        sources: &Sources,
        write_file: Option<Arc<dyn Fn(&Path, &[u8]) -> io::Result<()> + Send + Sync>>,
    ) -> RunOutput {
        let sink = SharedSink::buffer(false);
        let default_write: Arc<dyn Fn(&Path, &[u8]) -> io::Result<()> + Send + Sync> =
            Arc::new(|path, bytes| fs::write(path, bytes));
        let write_file = write_file.unwrap_or(default_write);
        run_collect(CollectParams {
            vault: &vault.root,
            inbox: &vault.inbox(),
            categories,
            since,
            day,
            clock: &clock,
            sink: &sink,
            gmail: &sources.gmail,
            messages: &sources.messages,
            calendar: &sources.calendar,
            write_file: write_file.as_ref(),
        })
    }

    #[test]
    fn worked_example_then_gmail_only() {
        let vault = TempVault::new();
        let gmail_item = test_item("thread", "Quarterly plan", at("2026-09-26T08:00:00-07:00"));
        let calendar_item = test_item("evt", "Dentist", at("2026-09-27T09:00:00-07:00"));
        let script = sources(vec![gmail_item], vec![], vec![calendar_item]);
        let output = exercise(
            &vault,
            clock("2026-09-27T10:00:00-07:00"),
            &Category::ALL,
            Some(day(2026, 9, 26)),
            None,
            &script,
            None,
        );
        assert_eq!(output.code, 0, "{}", output.stderr);
        assert!(output.stderr.is_empty());
        let mut lines = output.stdout.lines();
        assert!(lines.next().unwrap().ends_with("2026-09-27T100000-0700"));
        assert_eq!(lines.next(), Some("gmail=1"));
        assert_eq!(lines.next(), Some("messages=0"));
        assert_eq!(lines.next(), Some("calendar=1"));
        let run = vault.inbox().join("2026-09-27T100000-0700");
        assert!(run.join("gmail").join("Quarterly plan.md").is_file());
        assert!(run.join("calendar").join("Dentist.md").is_file());
        assert!(!run.join("messages").exists());
        let cursors = vault::load_cursors(&vault.root).unwrap();
        assert_eq!(cursors.gmail, Some(at("2026-09-27T10:00:00-07:00")));
        assert_eq!(cursors.messages, cursors.gmail);
        assert_eq!(cursors.calendar, cursors.gmail);

        let later = test_item("thread-2", "Afternoon", at("2026-09-27T12:00:00-07:00"));
        let script = sources(vec![later], vec![], vec![]);
        let output = exercise(
            &vault,
            clock("2026-09-27T18:00:00-07:00"),
            &[Category::Gmail],
            None,
            None,
            &script,
            None,
        );
        assert_eq!(output.code, 0, "{}", output.stderr);
        assert!(output
            .stdout
            .lines()
            .next()
            .unwrap()
            .ends_with("2026-09-27T180000-0700"));
        assert!(output.stdout.contains("gmail=1\n"));
        assert!(!output.stdout.contains("messages="));
        let cursors = vault::load_cursors(&vault.root).unwrap();
        assert_eq!(cursors.gmail, Some(at("2026-09-27T18:00:00-07:00")));
        assert_eq!(cursors.messages, Some(at("2026-09-27T10:00:00-07:00")));
        assert_eq!(cursors.calendar, Some(at("2026-09-27T10:00:00-07:00")));
        assert!(run.join("gmail").join("Quarterly plan.md").is_file());
        assert!(!run.join("gmail").join("Afternoon.md").exists());
        assert!(script.messages.calls.lock().unwrap().is_empty());
        assert!(script.calendar.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn three_date_layout_omits_empty_directories() {
        let vault = TempVault::new();
        let mut early = test_item("thread", "Monday", at("2026-09-25T08:00:00-07:00"));
        early.extra_front_matter = "account: early\n".into();
        early.body = "Monday body.\n".into();
        let mut late = test_item("thread", "Tuesday", at("2026-09-27T09:00:00-07:00"));
        late.extra_front_matter = "account: late\n".into();
        late.body = "Tuesday body.\n".into();
        late.source_url = early.source_url.clone();
        let event = test_item("evt", "Dentist", at("2026-09-27T09:30:00-07:00"));
        let script = sources(vec![late.clone(), early.clone()], vec![], vec![event]);
        let output = exercise(
            &vault,
            clock("2026-09-27T10:00:00-07:00"),
            &Category::ALL,
            Some(day(2026, 9, 25)),
            None,
            &script,
            None,
        );
        assert_eq!(output.code, 0, "{}", output.stderr);
        assert!(output.stdout.contains("2026-09-25 gmail=1\n"));
        assert!(output.stdout.contains("messages=0\n"));
        assert!(output.stdout.contains("2026-09-27 gmail=1\n"));
        assert!(output.stdout.contains("2026-09-27 calendar=1\n"));
        let run = vault.inbox().join("2026-09-27T100000-0700");
        assert!(run.join("2026-09-25/gmail/Monday.md").is_file());
        assert!(run.join("2026-09-27/gmail/Tuesday.md").is_file());
        assert!(run.join("2026-09-27/calendar/Dentist.md").is_file());
        assert!(!run.join("2026-09-26").exists());
        assert!(!run.join("messages").exists());
        assert!(!run.join("2026-09-25/messages").exists());

        let mut monday = test_item("thread", "Monday", at("2026-09-26T08:00:00-07:00"));
        monday.extra_front_matter = "account: early\n".into();
        monday.body = "Monday body.\n".into();
        let mut tuesday = test_item("thread", "Tuesday", at("2026-09-27T09:00:00-07:00"));
        tuesday.extra_front_matter = "account: late\n".into();
        tuesday.body = "Tuesday body.\n".into();
        let flat_script = sources(vec![tuesday, monday], vec![], vec![]);
        let flat_vault = TempVault::new();
        let flat = exercise(
            &flat_vault,
            clock("2026-09-27T10:00:00-07:00"),
            &[Category::Gmail],
            Some(day(2026, 9, 26)),
            None,
            &flat_script,
            None,
        );
        assert_eq!(flat.code, 0, "{}", flat.stderr);
        let flat_run = flat_vault.inbox().join("2026-09-27T100000-0700");
        let text = fs::read_to_string(flat_run.join("gmail/Tuesday.md")).unwrap();
        assert!(text.contains("account: early\n"));
        assert!(!text.contains("account: late"));
        assert!(text.contains("[Open in Gmail](https://example.test/thread)"));
        assert!(text.contains("Monday body.\n\nTuesday body.\n"));
        assert!(text.contains("timezone: America/Los_Angeles"));
        assert!(!flat_run.join("2026-09-26").exists());
    }

    #[test]
    fn empty_success_and_cursor_equal_to_the_end() {
        let vault = TempVault::new();
        let script = sources(vec![], vec![], vec![]);
        let output = exercise(
            &vault,
            clock("2026-09-27T10:00:00-07:00"),
            &Category::ALL,
            Some(day(2026, 9, 26)),
            None,
            &script,
            None,
        );
        assert_eq!(output.code, 0, "{}", output.stderr);
        assert_eq!(output.stdout, "gmail=0\nmessages=0\ncalendar=0\n");
        assert!(!vault.inbox().exists());
        assert_eq!(
            vault::load_cursors(&vault.root).unwrap().gmail,
            Some(at("2026-09-27T10:00:00-07:00"))
        );

        let before = fs::read(vault::state_path(&vault.root)).unwrap();
        script.gmail.calls.lock().unwrap().clear();
        let output = exercise(
            &vault,
            clock("2026-09-27T10:00:00-07:00"),
            &[Category::Gmail],
            None,
            None,
            &script,
            None,
        );
        assert_eq!(output.code, 0, "{}", output.stderr);
        assert_eq!(output.stdout, "gmail=0\n");
        assert!(script.gmail.calls.lock().unwrap().is_empty());
        assert_eq!(fs::read(vault::state_path(&vault.root)).unwrap(), before);
    }

    #[test]
    fn rejections_write_nothing_and_release_the_lock() {
        let vault = TempVault::new();
        let mut cursors = Cursors::default();
        cursors.gmail = Some(at("2026-09-27T18:00:00-07:00"));
        cursors.messages = Some(at("2026-09-27T10:00:00-07:00"));
        cursors.calendar = Some(at("2026-09-27T10:00:00-07:00"));
        vault::store_cursors(&vault.root, &cursors).unwrap();
        let before = fs::read(vault::state_path(&vault.root)).unwrap();
        let script = sources(vec![], vec![], vec![]);
        let backward = exercise(
            &vault,
            clock("2026-09-27T10:00:00-07:00"),
            &[Category::Gmail],
            None,
            None,
            &script,
            None,
        );
        assert_eq!(backward.code, 1);
        assert!(backward
            .stderr
            .contains("frozen end is earlier than the gmail cursor"));
        assert!(!vault.inbox().exists());
        assert_eq!(fs::read(vault::state_path(&vault.root)).unwrap(), before);
        assert!(!vault::lock_path(&vault.root).exists());

        let since = exercise(
            &vault,
            clock("2026-09-27T18:00:00-07:00"),
            &Category::ALL,
            Some(day(2026, 9, 26)),
            None,
            &script,
            None,
        );
        assert_eq!(since.code, 1);
        assert!(since
            .stderr
            .contains("not allowed when every selected category has a cursor"));

        let fresh = TempVault::new();
        let after = exercise(
            &fresh,
            clock("2026-09-27T10:00:00-07:00"),
            &[Category::Gmail],
            Some(day(2026, 9, 28)),
            None,
            &script,
            None,
        );
        assert_eq!(after.code, 1);
        assert!(after.stderr.contains("--since is after the run ends"));
        let missing = exercise(
            &fresh,
            clock("2026-09-27T10:00:00-07:00"),
            &[Category::Gmail],
            None,
            None,
            &script,
            None,
        );
        assert_eq!(missing.code, 1);
        assert!(missing.stderr.contains("--since is required"));
    }

    #[test]
    fn fetch_failure_leaves_no_folder_and_stops_later_jobs() {
        let vault = TempVault::new();
        let gmail = MapSource::failing(day(2026, 9, 25), "boom");
        let calls = gmail.calls.clone();
        let message = test_item("chat", "Ada", at("2026-09-25T08:00:00-07:00"));
        let script = Sources {
            gmail,
            messages: MapSource::new(vec![message]),
            calendar: MapSource::new(vec![]),
        };
        let output = exercise(
            &vault,
            clock("2026-09-27T10:00:00-07:00"),
            &Category::ALL,
            Some(day(2026, 9, 25)),
            None,
            &script,
            None,
        );
        assert_eq!(output.code, 2);
        assert_eq!(output.stderr, "2026-09-25 failed gmail: boom\n");
        assert!(!vault.inbox().exists());
        assert!(vault::load_cursors(&vault.root).unwrap().gmail.is_none());
        assert!(!vault::lock_path(&vault.root).exists());
        let seen = calls.lock().unwrap().clone();
        assert_eq!(seen, vec![day(2026, 9, 25)]);
    }

    #[test]
    fn write_failure_keeps_cursors_and_can_leave_a_folder() {
        let vault = TempVault::new();
        let first = test_item("a", "Alpha", at("2026-09-26T08:00:00-07:00"));
        let second = test_item("b", "Beta", at("2026-09-26T09:00:00-07:00"));
        let script = sources(vec![first, second], vec![], vec![]);
        let writes = Arc::new(AtomicBool::new(false));
        let flag = writes.clone();
        let write_file: Arc<dyn Fn(&Path, &[u8]) -> io::Result<()> + Send + Sync> =
            Arc::new(move |path, bytes| {
                if flag.swap(true, Ordering::SeqCst) {
                    Err(io::Error::other("disk full"))
                } else {
                    fs::write(path, bytes)
                }
            });
        let output = exercise(
            &vault,
            clock("2026-09-27T10:00:00-07:00"),
            &[Category::Gmail],
            Some(day(2026, 9, 26)),
            None,
            &script,
            Some(write_file),
        );
        assert_eq!(output.code, 2);
        assert!(output
            .stderr
            .contains("failed gmail: write failed: disk full"));
        assert!(vault.inbox().join("2026-09-27T100000-0700").is_dir());
        assert!(vault::load_cursors(&vault.root).unwrap().gmail.is_none());
        assert!(!writes.load(Ordering::SeqCst) || vault.inbox().exists());
    }

    #[test]
    fn day_writes_a_folder_without_moving_cursors() {
        let vault = TempVault::new();
        let mut cursors = Cursors::default();
        cursors.gmail = Some(at("2026-09-27T18:00:00-07:00"));
        vault::store_cursors(&vault.root, &cursors).unwrap();
        let before = fs::read(vault::state_path(&vault.root)).unwrap();
        let item = test_item("thread", "Replay", at("2026-09-26T08:00:00-07:00"));
        let script = sources(vec![item], vec![], vec![]);
        let output = exercise(
            &vault,
            clock("2026-09-27T10:00:00-07:00"),
            &[Category::Gmail],
            None,
            Some(day(2026, 9, 26)),
            &script,
            None,
        );
        assert_eq!(output.code, 0, "{}", output.stderr);
        assert!(output
            .stdout
            .lines()
            .next()
            .unwrap()
            .ends_with("2026-09-27T100000-0700"));
        assert!(output.stdout.contains("gmail=1\n"));
        assert!(vault
            .inbox()
            .join("2026-09-27T100000-0700/gmail/Replay.md")
            .is_file());
        assert_eq!(fs::read(vault::state_path(&vault.root)).unwrap(), before);
    }

    #[test]
    fn existing_lock_is_left_in_place() {
        let vault = TempVault::new();
        fs::create_dir_all(vault.root.join(".pkmagent")).unwrap();
        fs::write(vault::lock_path(&vault.root), "busy\n").unwrap();
        let script = sources(vec![], vec![], vec![]);
        let output = exercise(
            &vault,
            clock("2026-09-27T10:00:00-07:00"),
            &[Category::Gmail],
            Some(day(2026, 9, 26)),
            None,
            &script,
            None,
        );
        assert_eq!(output.code, 1);
        assert!(output.stderr.contains("delete it when nothing is running"));
        assert_eq!(
            fs::read_to_string(vault::lock_path(&vault.root)).unwrap(),
            "busy\n"
        );
        assert!(!vault.inbox().exists());
    }

    #[test]
    fn corrupt_state_exits_1_and_skips_a_later_category_slice() {
        let vault = TempVault::new();
        fs::create_dir_all(vault.root.join(".pkmagent")).unwrap();
        fs::write(vault::state_path(&vault.root), "[1,2]").unwrap();
        let script = sources(vec![], vec![], vec![]);
        let output = exercise(
            &vault,
            clock("2026-09-27T10:00:00-07:00"),
            &[Category::Gmail],
            Some(day(2026, 9, 26)),
            None,
            &script,
            None,
        );
        assert_eq!(output.code, 1);
        assert!(output.stderr.contains("state.json is corrupt"));
        assert!(!vault.inbox().exists());
        assert!(!vault::lock_path(&vault.root).exists());

        let mut outside = test_item("old", "Old", at("2026-09-25T08:00:00-07:00"));
        outside.stable_id = "keep".into();
        let inside = test_item("keep", "Kept", at("2026-09-26T08:00:00-07:00"));
        let mut gmail_cursor = Cursors::default();
        gmail_cursor.gmail = Some(at("2026-09-27T10:00:00-07:00"));
        let vault = TempVault::new();
        vault::store_cursors(&vault.root, &gmail_cursor).unwrap();
        let script = sources(
            vec![outside, inside],
            vec![test_item("m", "Msg", at("2026-09-26T01:00:00-07:00"))],
            vec![],
        );
        let output = exercise(
            &vault,
            clock("2026-09-27T10:00:00-07:00"),
            &[Category::Gmail, Category::Messages],
            Some(day(2026, 9, 26)),
            None,
            &script,
            None,
        );
        assert_eq!(output.code, 0, "{}", output.stderr);
        assert!(script.gmail.calls.lock().unwrap().is_empty());
        assert_eq!(script.messages.calls.lock().unwrap().len(), 2);
        let run = vault.inbox().join("2026-09-27T100000-0700");
        assert!(!run.join("gmail").exists());
        assert!(run.join("messages/Msg.md").is_file());
        let text = fs::read_to_string(run.join("messages/Msg.md")).unwrap();
        assert!(!text.contains("Old"));
    }

    #[test]
    fn run_folder_suffix_and_stdout_order_follow_category_order() {
        let vault = TempVault::new();
        let stamp = "2026-09-27T100000-0700";
        fs::create_dir_all(vault.inbox().join(stamp)).unwrap();
        fs::write(vault.inbox().join(stamp).join("keep.txt"), b"keep").unwrap();
        let gmail = test_item("g", "Mail", at("2026-09-26T08:00:00-07:00"));
        let calendar = test_item("c", "Event", at("2026-09-26T09:00:00-07:00"));
        let script = sources(vec![gmail], vec![], vec![calendar]);
        let output = exercise(
            &vault,
            clock("2026-09-27T10:00:00-07:00"),
            &[Category::Calendar, Category::Gmail],
            Some(day(2026, 9, 26)),
            None,
            &script,
            None,
        );
        assert_eq!(output.code, 0, "{}", output.stderr);
        let lines: Vec<_> = output.stdout.lines().collect();
        assert!(lines[0].ends_with("2026-09-27T100000-0700-2"));
        assert_eq!(lines[1], "gmail=1");
        assert_eq!(lines[2], "calendar=1");
        assert_eq!(
            fs::read(vault.inbox().join(stamp).join("keep.txt")).unwrap(),
            b"keep"
        );
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn import_on_linux_fails_without_writing() {
        let vault = TempVault::new();
        let sink = SharedSink::buffer(false);
        let output = run_import(&vault.root, &clock("2026-09-27T10:00:00-07:00"), &sink);
        assert_eq!(output.code, 2);
        assert_eq!(
            output.stderr,
            "contacts failed: Contacts is only available on macOS\n"
        );
        assert!(output.stdout.is_empty());
        assert!(!vault.inbox().exists());
        assert!(!vault.root.join("contacts").exists());
        assert!(!vault::lock_path(&vault.root).exists());
    }
}
