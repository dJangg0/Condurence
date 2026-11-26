//! Per-provider token usage, read from each CLI's own session logs.
//!
//! Condurence never sees a provider's credentials (NFR-SEC-02), and headless output is not
//! enough: the root agent is an interactive TUI that prints no usage. So the meter reads the
//! usage counters each CLI already writes to its local session files.
//!
//! Two scopes use the same reader:
//! - A run's meter counts sessions started inside the run's working directories during the run.
//!   Attribution is by directory and time, so a session the user opens by hand in the same
//!   worktree during a run is counted too.
//! - The profile pane counts every session on this Mac that started in the last period.
//!
//! A session is counted in the period it started, so a session that began before the window and
//! kept working into it is not split. Providers whose logs are not read yet report
//! `measured: false` instead of a misleading zero.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

use crate::runtime::state_db::RunUsageInputs;

/// Clock slack when comparing a session's timestamps with the window.
const WINDOW_SLACK_MS: i64 = 2_000;
const MS_PER_DAY: i64 = 86_400_000;

/// Providers the meter knows about, in display order.
pub const PROVIDERS: [&str; 4] = ["claude", "codex", "opencode", "gemini"];

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    /// CLI binary name, such as `claude`.
    pub provider: String,
    /// False when this CLI's logs are not read yet, so the counters below are not meaningful.
    pub measured: bool,
    /// Session files attributed to the run.
    pub sessions: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

/// Which sessions count: those started inside `[start_ms, end_ms]`, and, when `cwds` is set, only
/// those that ran in one of the directories.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    /// Working directories to count, or `None` to count sessions from every directory.
    pub cwds: Option<Vec<PathBuf>>,
    pub start_ms: i64,
    pub end_ms: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Tokens {
    fn add(&mut self, other: Tokens) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

/// What one session file says about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionRecord {
    /// Working directory the CLI reported for the session.
    pub cwd: Option<String>,
    /// Epoch ms of the session's earliest timestamp.
    pub started_ms: Option<i64>,
    pub tokens: Tokens,
}

/// Reads session logs and remembers what it parsed. A file is parsed again only when its length or
/// modification time changes, so the live run meter and the profile pane stay cheap to refresh.
#[derive(Default)]
pub struct UsageReader {
    cache: Mutex<HashMap<PathBuf, CachedSession>>,
}

#[derive(Clone)]
struct CachedSession {
    len: u64,
    modified_ms: i64,
    record: SessionRecord,
}

impl UsageReader {
    /// Usage for each listed provider that has sessions in the window, in `PROVIDERS` order.
    pub fn measure(&self, home: &Path, providers: &[String], window: &Window) -> Vec<ProviderUsage> {
        let cwds = cwd_set(window);
        let mut rows = Vec::new();
        for provider in PROVIDERS {
            if !providers.iter().any(|used| used == provider) {
                continue;
            }
            let mut row = ProviderUsage {
                provider: provider.into(),
                measured: true,
                sessions: 0,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            };
            let records = match provider {
                "claude" => self.claude_sessions(home, window),
                "codex" => self.codex_sessions(home, window),
                _ => {
                    row.measured = false;
                    Vec::new()
                }
            };
            for record in records.iter().filter(|record| belongs_to(record, cwds.as_ref(), window)) {
                row.sessions += 1;
                row.input_tokens += record.tokens.input;
                row.output_tokens += record.tokens.output;
                row.cache_read_tokens += record.tokens.cache_read;
                row.cache_write_tokens += record.tokens.cache_write;
            }
            rows.push(row);
        }
        rows
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<PathBuf, CachedSession>> {
        self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Parses a session file, or reuses the earlier parse when the file has not changed.
    fn read_record(&self, path: &Path, parse: fn(&str) -> SessionRecord) -> Option<SessionRecord> {
        let meta = fs::metadata(path).ok()?;
        let len = meta.len();
        let modified_ms = millis_since_epoch(meta.modified().ok()?)?;
        let cached = {
            let entries = self.entries();
            entries
                .get(path)
                .filter(|hit| hit.len == len && hit.modified_ms == modified_ms)
                .map(|hit| hit.record.clone())
        };
        if cached.is_some() {
            return cached;
        }
        let record = parse(&fs::read_to_string(path).ok()?);
        self.entries().insert(
            path.to_path_buf(),
            CachedSession {
                len,
                modified_ms,
                record: record.clone(),
            },
        );
        Some(record)
    }
}

/// Days covered by the profile pane's token usage summary.
pub const SUMMARY_DAYS: i64 = 30;

/// Usage for one run, as the run meter reads it. An active run (no end time yet) counts up to
/// `now_ms`.
pub fn measure_run(reader: &UsageReader, home: &Path, run: &RunUsageInputs, now_ms: i64) -> Vec<ProviderUsage> {
    let window = Window {
        cwds: Some(run.cwds.iter().map(PathBuf::from).collect()),
        start_ms: run.started_ms,
        end_ms: run.ended_ms.unwrap_or(now_ms),
    };
    reader.measure(home, &run.commands, &window)
}

/// Usage for every provider across all sessions that started in the last `days` days.
pub fn summarize(reader: &UsageReader, home: &Path, days: i64, now_ms: i64) -> Vec<ProviderUsage> {
    let providers: Vec<String> = PROVIDERS.iter().map(|provider| provider.to_string()).collect();
    let window = Window {
        cwds: None,
        start_ms: now_ms - days * MS_PER_DAY,
        end_ms: now_ms,
    };
    reader.measure(home, &providers, &window)
}

/// Canonical directories a window is limited to, or `None` when every directory counts.
fn cwd_set(window: &Window) -> Option<BTreeSet<String>> {
    window
        .cwds
        .as_ref()
        .map(|paths| paths.iter().map(|path| canonical_string(path)).collect())
}

fn belongs_to(record: &SessionRecord, cwds: Option<&BTreeSet<String>>, window: &Window) -> bool {
    let in_directory = match cwds {
        None => true,
        Some(set) => record
            .cwd
            .as_deref()
            .is_some_and(|cwd| set.contains(&canonical_string(Path::new(cwd)))),
    };
    let in_window = record
        .started_ms
        .is_some_and(|started| started >= window.start_ms - WINDOW_SLACK_MS && started <= window.end_ms + WINDOW_SLACK_MS);
    in_directory && in_window
}

// ---------------------------------------------------------------- Claude Code

/// Claude keeps one `.jsonl` per session under `~/.claude/projects/<encoded cwd>/`.
pub fn claude_project_dir_name(cwd: &str) -> String {
    cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

impl UsageReader {
    /// Only the run's project directories are read. Without a directory filter, every project
    /// directory is read, since any of them may hold a session from the window.
    fn claude_sessions(&self, home: &Path, window: &Window) -> Vec<SessionRecord> {
        let projects = home.join(".claude").join("projects");
        let dirs: BTreeSet<PathBuf> = match &window.cwds {
            Some(cwds) => cwds
                .iter()
                .map(|cwd| projects.join(claude_project_dir_name(&canonical_string(cwd))))
                .collect(),
            None => match fs::read_dir(&projects) {
                Ok(entries) => entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.is_dir())
                    .collect(),
                Err(_) => BTreeSet::new(),
            },
        };
        let mut records = Vec::new();
        for dir in dirs {
            for path in jsonl_files(&dir, window) {
                if let Some(record) = self.read_record(&path, parse_claude_session) {
                    records.push(record);
                }
            }
        }
        records
    }
}

/// Reads a Claude session. Each reply is written as several lines that all repeat the same usage,
/// so usage is kept once per `requestId`. Most of a session is tool output and prose, so only
/// lines that mention usage, or lines still needed for identity, are parsed as JSON.
pub fn parse_claude_session(text: &str) -> SessionRecord {
    let mut record = SessionRecord::default();
    let mut per_request: HashMap<String, Tokens> = HashMap::new();
    let mut unkeyed = Tokens::default();
    for line in text.lines() {
        let needs_identity = record.cwd.is_none() || record.started_ms.is_none();
        let has_usage = line.contains("\"usage\"");
        if !needs_identity && !has_usage {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else { continue };
        if needs_identity {
            note_identity(&mut record, &value);
        }
        if !has_usage || value.get("type").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let message = value.get("message").unwrap_or(&Value::Null);
        let Some(usage) = message.get("usage").filter(|usage| usage.is_object()) else { continue };
        let tokens = claude_tokens(usage);
        match value.get("requestId").or_else(|| message.get("id")).and_then(Value::as_str) {
            Some(id) => {
                per_request.insert(id.to_string(), tokens);
            }
            None => unkeyed.add(tokens),
        }
    }
    let mut total = unkeyed;
    for tokens in per_request.values() {
        total.add(*tokens);
    }
    record.tokens = total;
    record
}

fn claude_tokens(usage: &Value) -> Tokens {
    let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    Tokens {
        input: count("input_tokens"),
        output: count("output_tokens"),
        cache_read: count("cache_read_input_tokens"),
        cache_write: count("cache_creation_input_tokens"),
    }
}

// ---------------------------------------------------------------- Codex CLI

/// Codex writes one rollout per session under `~/.codex/sessions/YYYY/MM/DD/`. The directory date
/// may not match the UTC day the run used, so the search widens the window by a day on each side.
impl UsageReader {
    fn codex_sessions(&self, home: &Path, window: &Window) -> Vec<SessionRecord> {
        let cwds = cwd_set(window);
        let first_day = (window.start_ms - WINDOW_SLACK_MS).div_euclid(MS_PER_DAY) - 1;
        let last_day = window.end_ms.div_euclid(MS_PER_DAY) + 1;
        let root = home.join(".codex").join("sessions");
        let mut records = Vec::new();
        for day in first_day..=last_day {
            let (year, month, date) = civil_from_days(day);
            let dir = root.join(format!("{year:04}")).join(format!("{month:02}")).join(format!("{date:02}"));
            for path in jsonl_files(&dir, window) {
                let is_rollout = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("rollout-"));
                // Rollouts outside the window's directories are rejected from their first line,
                // so only the sessions that count are read in full.
                if is_rollout && codex_head_belongs(&path, cwds.as_ref(), window) {
                    if let Some(record) = self.read_record(&path, parse_codex_rollout) {
                        records.push(record);
                    }
                }
            }
        }
        records
    }
}

/// The first line of a rollout is its session metadata, so the directory and start time can be
/// checked without reading the rest of the file.
fn codex_head_belongs(path: &Path, cwds: Option<&BTreeSet<String>>, window: &Window) -> bool {
    let Ok(file) = fs::File::open(path) else { return false };
    let mut first = String::new();
    if BufReader::new(file).read_line(&mut first).is_err() {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(&first) else { return false };
    let mut head = SessionRecord::default();
    note_identity(&mut head, &value);
    head.cwd = value.pointer("/payload/cwd").and_then(Value::as_str).map(str::to_string);
    belongs_to(&head, cwds, window)
}

/// Reads a Codex rollout. `total_token_usage` is a running total for the whole session, so the
/// last report is the session's usage. Codex counts cached tokens inside `input_tokens`, so they
/// are split out here to keep fresh input and cache reads apart.
pub fn parse_codex_rollout(text: &str) -> SessionRecord {
    let mut record = SessionRecord::default();
    let mut latest: Option<Tokens> = None;
    for line in text.lines() {
        let needs_identity = record.cwd.is_none() || record.started_ms.is_none();
        let mentions_tokens = line.contains("token_count");
        if !needs_identity && !mentions_tokens {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else { continue };
        let payload = value.get("payload").unwrap_or(&Value::Null);
        if needs_identity {
            note_identity(&mut record, &value);
            if record.cwd.is_none() {
                record.cwd = payload.get("cwd").and_then(Value::as_str).map(str::to_string);
            }
        }
        let is_token_count = value.get("type").and_then(Value::as_str) == Some("event_msg")
            && payload.get("type").and_then(Value::as_str) == Some("token_count");
        if is_token_count {
            if let Some(total) = payload.pointer("/info/total_token_usage") {
                latest = Some(codex_tokens(total));
            }
        }
    }
    record.tokens = latest.unwrap_or_default();
    record
}

fn codex_tokens(total: &Value) -> Tokens {
    let count = |key: &str| total.get(key).and_then(Value::as_u64).unwrap_or(0);
    let cached = count("cached_input_tokens");
    Tokens {
        input: count("input_tokens").saturating_sub(cached),
        output: count("output_tokens"),
        cache_read: cached,
        cache_write: count("cache_write_input_tokens"),
    }
}

// ---------------------------------------------------------------- shared helpers

/// Records the first working directory and the earliest timestamp seen in a line.
fn note_identity(record: &mut SessionRecord, value: &Value) {
    if record.cwd.is_none() {
        record.cwd = value.get("cwd").and_then(Value::as_str).map(str::to_string);
    }
    let timestamp = value
        .get("timestamp")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/payload/timestamp").and_then(Value::as_str))
        .and_then(parse_iso_ms);
    if let Some(ms) = timestamp {
        record.started_ms = Some(record.started_ms.map_or(ms, |current| current.min(ms)));
    }
}

/// `.jsonl` files in `dir` that were modified at or after the window opened. A session that
/// started inside the window is always modified inside it, so older files are skipped unread.
fn jsonl_files(dir: &Path, window: &Window) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .filter(|path| {
            fs::metadata(path)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(millis_since_epoch)
                .is_some_and(|ms| ms >= window.start_ms - WINDOW_SLACK_MS)
        })
        .collect()
}

fn millis_since_epoch(time: SystemTime) -> Option<i64> {
    let elapsed = time.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(elapsed.as_millis()).ok()
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn canonical_string(path: &Path) -> String {
    canonical(path).to_string_lossy().into_owned()
}

/// Epoch ms for an ISO-8601 UTC timestamp such as `2026-11-25T18:45:24.082Z`. Other offsets are
/// rejected, since both CLIs write UTC.
pub fn parse_iso_ms(text: &str) -> Option<i64> {
    let body = text.strip_suffix('Z')?;
    let (date, time) = body.split_once('T')?;
    let mut date_parts = date.splitn(3, '-').map(|part| part.parse::<i64>());
    let year = date_parts.next()?.ok()?;
    let month = date_parts.next()?.ok()?;
    let day = date_parts.next()?.ok()?;
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let mut clock_parts = clock.splitn(3, ':').map(|part| part.parse::<i64>());
    let hour = clock_parts.next()?.ok()?;
    let minute = clock_parts.next()?.ok()?;
    let second = clock_parts.next()?.ok()?;
    let millis: i64 = format!("{fraction:0<3}").get(..3)?.parse().ok()?;
    let seconds = (hour * 60 + minute) * 60 + second;
    Some(days_from_civil(year, month, day) * MS_PER_DAY + seconds * 1_000 + millis)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y.rem_euclid(400);
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Inverse of `days_from_civil`: `(year, month, day)` for a day count since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude_line(request: &str, input: u64, output: u64, cache_read: u64, cache_write: u64) -> String {
        format!(
            r#"{{"type":"assistant","requestId":"{request}","cwd":"/work/repo","timestamp":"2026-11-25T18:45:24.082Z","message":{{"id":"m-{request}","usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":{cache_read},"cache_creation_input_tokens":{cache_write}}}}}}}"#
        )
    }

    #[test]
    fn claude_reply_repeated_across_content_blocks_is_counted_once() {
        let text = [
            r#"{"type":"user","cwd":"/work/repo","timestamp":"2026-11-25T18:45:20.000Z","message":{"role":"user"}}"#.to_string(),
            claude_line("req-1", 10, 100, 5_000, 300),
            claude_line("req-1", 10, 100, 5_000, 300),
            claude_line("req-1", 10, 100, 5_000, 300),
            claude_line("req-2", 4, 50, 6_000, 0),
        ]
        .join("\n");
        let record = parse_claude_session(&text);
        assert_eq!(record.cwd.as_deref(), Some("/work/repo"));
        assert_eq!(record.tokens, Tokens { input: 14, output: 150, cache_read: 11_000, cache_write: 300 });
        assert_eq!(record.started_ms, parse_iso_ms("2026-11-25T18:45:20.000Z"));
    }

    #[test]
    fn claude_lines_without_usage_or_request_id_are_still_summed() {
        let text = r#"{"type":"assistant","message":{"usage":{"input_tokens":3,"output_tokens":4}}}
not json at all
{"type":"assistant","message":{"usage":{"input_tokens":1,"output_tokens":2}}}"#;
        let record = parse_claude_session(text);
        assert_eq!(record.tokens.input, 4);
        assert_eq!(record.tokens.output, 6);
        assert_eq!(record.cwd, None);
    }

    #[test]
    fn codex_totals_are_cumulative_and_cached_input_is_split_out() {
        let text = [
            r#"{"timestamp":"2026-11-20T07:40:19.764Z","type":"session_meta","payload":{"cwd":"/work/repo","timestamp":"2026-11-20T07:40:19.764Z"}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":100,"cache_write_input_tokens":0}}}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":3000,"cached_input_tokens":2500,"output_tokens":250,"cache_write_input_tokens":7}}}}"#,
        ]
        .join("\n");
        let record = parse_codex_rollout(&text);
        assert_eq!(record.cwd.as_deref(), Some("/work/repo"));
        assert_eq!(record.started_ms, parse_iso_ms("2026-11-20T07:40:19.764Z"));
        assert_eq!(record.tokens, Tokens { input: 500, output: 250, cache_read: 2_500, cache_write: 7 });
    }

    #[test]
    fn codex_rollout_without_token_counts_reports_zero() {
        let text = r#"{"timestamp":"2026-11-20T07:40:19.764Z","type":"session_meta","payload":{"cwd":"/work/repo"}}"#;
        assert_eq!(parse_codex_rollout(text).tokens, Tokens::default());
    }

    #[test]
    fn claude_directory_names_replace_every_non_alphanumeric_character() {
        // Matches the directory names Claude wrote for real sessions, including `/.worktrees/`.
        assert_eq!(
            claude_project_dir_name("/Users/me/repo/.worktrees/05192e75-greeter-1"),
            "-Users-me-repo--worktrees-05192e75-greeter-1"
        );
        assert_eq!(claude_project_dir_name("/a_b/c.d"), "-a-b-c-d");
    }

    #[test]
    fn iso_timestamps_convert_to_epoch_millis() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:01.500Z"), Some(1_500));
        assert_eq!(parse_iso_ms("1970-01-02T00:00:00Z"), Some(86_400_000));
        assert_eq!(parse_iso_ms("2024-02-29T12:00:00.1Z"), Some(parse_iso_ms("2024-02-29T12:00:00.100Z").unwrap()));
        assert_eq!(parse_iso_ms("2026-11-25T18:45:24.082+02:00"), None);
        assert_eq!(parse_iso_ms("yesterday"), None);
    }

    #[test]
    fn civil_dates_round_trip_through_day_numbers() {
        for (year, month, day) in [(1970, 1, 1), (2000, 2, 29), (2024, 2, 29), (2026, 11, 26), (1999, 12, 31)] {
            let days = days_from_civil(year, month, day);
            assert_eq!(civil_from_days(days), (year, month, day));
        }
        assert_eq!(days_from_civil(1970, 1, 1), 0);
    }

    /// ISO-8601 UTC text for an epoch-ms value, as the CLIs write it.
    fn format_iso(ms: i64) -> String {
        let (days, rest) = (ms.div_euclid(MS_PER_DAY), ms.rem_euclid(MS_PER_DAY));
        let (y, mo, d) = civil_from_days(days);
        let (h, mi, s, milli) = (rest / 3_600_000, rest / 60_000 % 60, rest / 1_000 % 60, rest % 1_000);
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{milli:03}Z")
    }

    fn now_ms() -> i64 {
        std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
    }

    #[test]
    fn measure_counts_only_sessions_in_run_directories_and_window() {
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("repo");
        let other = home.path().join("elsewhere");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&other).unwrap();
        let start = now_ms() - 60_000;
        let end = now_ms() + 60_000;

        // Claude: one session in the run, one from before the run, one in a different directory.
        let repo_name = claude_project_dir_name(&canonical_string(&repo));
        let claude_dir = home.path().join(".claude/projects").join(repo_name);
        fs::create_dir_all(&claude_dir).unwrap();
        let claude_session = |ms: i64, request: &str| {
            format!(
                r#"{{"type":"assistant","requestId":"{request}","cwd":"{}","timestamp":"{}","message":{{"usage":{{"input_tokens":100,"output_tokens":10,"cache_read_input_tokens":99,"cache_creation_input_tokens":5}}}}}}"#,
                repo.display(),
                format_iso(ms)
            )
        };
        fs::write(claude_dir.join("in-run.jsonl"), claude_session(start + 5_000, "a")).unwrap();
        fs::write(claude_dir.join("before-run.jsonl"), claude_session(start - 3_600_000, "b")).unwrap();
        let other_dir = home.path().join(".claude/projects").join("-elsewhere");
        fs::create_dir_all(&other_dir).unwrap();
        let unrelated = claude_session(start + 5_000, "c").replace(&repo.display().to_string(), &other.display().to_string());
        fs::write(other_dir.join("unrelated.jsonl"), unrelated).unwrap();

        // Codex: one rollout in the run's repo, one in another directory, both dated today (UTC).
        let (year, month, day) = civil_from_days(now_ms().div_euclid(MS_PER_DAY));
        let codex_dir = home.path().join(format!(".codex/sessions/{year:04}/{month:02}/{day:02}"));
        fs::create_dir_all(&codex_dir).unwrap();
        let rollout = |cwd: &Path| {
            let meta = format!(
                r#"{{"timestamp":"{t}","type":"session_meta","payload":{{"cwd":"{c}","timestamp":"{t}"}}}}"#,
                t = format_iso(start + 1_000),
                c = cwd.display()
            );
            let usage = r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":50,"cached_input_tokens":20,"output_tokens":8,"cache_write_input_tokens":0}}}}"#;
            format!("{meta}\n{usage}")
        };
        fs::write(codex_dir.join("rollout-run.jsonl"), rollout(&repo)).unwrap();
        fs::write(codex_dir.join("rollout-other.jsonl"), rollout(&other)).unwrap();

        let providers = vec!["codex".to_string(), "claude".into(), "gemini".into()];
        let window = Window { cwds: Some(vec![repo.clone()]), start_ms: start, end_ms: end };
        let rows = UsageReader::default().measure(home.path(), &providers, &window);

        // Providers appear in PROVIDERS order, and opencode never ran so it has no row.
        assert_eq!(rows.iter().map(|row| row.provider.as_str()).collect::<Vec<_>>(), ["claude", "codex", "gemini"]);

        let claude = &rows[0];
        assert!(claude.measured);
        assert_eq!(claude.sessions, 1);
        assert_eq!((claude.input_tokens, claude.output_tokens, claude.cache_write_tokens), (100, 10, 5));
        assert_eq!(claude.cache_read_tokens, 99);

        let codex = &rows[1];
        assert!(codex.measured);
        assert_eq!(codex.sessions, 1);
        assert_eq!((codex.input_tokens, codex.cache_read_tokens, codex.output_tokens), (30, 20, 8));

        let gemini = &rows[2];
        assert!(!gemini.measured, "gemini logs are not read yet");
        assert_eq!(gemini.sessions, 0);
    }

    #[test]
    fn missing_home_directories_yield_empty_measured_rows() {
        let home = tempfile::tempdir().unwrap();
        let providers = vec!["claude".to_string(), "codex".into()];
        let window = Window { cwds: Some(vec![home.path().to_path_buf()]), start_ms: 0, end_ms: now_ms() };
        let rows = UsageReader::default().measure(home.path(), &providers, &window);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.measured && row.sessions == 0 && row.input_tokens == 0));
    }

    #[test]
    fn summary_without_a_directory_filter_counts_every_directory_started_in_the_window() {
        let home = tempfile::tempdir().unwrap();
        let (alpha, beta) = (home.path().join("alpha"), home.path().join("beta"));
        fs::create_dir_all(&alpha).unwrap();
        fs::create_dir_all(&beta).unwrap();
        let now = now_ms();
        let start = now - 30 * MS_PER_DAY;

        // Two Claude project directories, one session each inside the window, plus one from long ago.
        for (dir, cwd, when, request) in [
            (&alpha, &alpha, now - 3_600_000, "a"),
            (&beta, &beta, now - 7_200_000, "b"),
            (&alpha, &alpha, start - 90 * MS_PER_DAY, "old"),
        ] {
            let name = claude_project_dir_name(&canonical_string(dir));
            let project = home.path().join(".claude/projects").join(name);
            fs::create_dir_all(&project).unwrap();
            let line = format!(
                r#"{{"type":"assistant","requestId":"{request}","cwd":"{}","timestamp":"{}","message":{{"usage":{{"input_tokens":10,"output_tokens":1}}}}}}"#,
                cwd.display(),
                format_iso(when)
            );
            fs::write(project.join(format!("{request}.jsonl")), line).unwrap();
        }

        // Codex: one rollout per directory, both started today.
        let (year, month, day) = civil_from_days(now.div_euclid(MS_PER_DAY));
        let codex_dir = home.path().join(format!(".codex/sessions/{year:04}/{month:02}/{day:02}"));
        fs::create_dir_all(&codex_dir).unwrap();
        for cwd in [&alpha, &beta] {
            let meta = format!(
                r#"{{"timestamp":"{t}","type":"session_meta","payload":{{"cwd":"{c}","timestamp":"{t}"}}}}"#,
                t = format_iso(now - 1_800_000),
                c = cwd.display()
            );
            let usage = r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":5,"cached_input_tokens":0,"output_tokens":2}}}}"#;
            fs::write(codex_dir.join(format!("rollout-{}.jsonl", cwd.file_name().unwrap().to_string_lossy())), format!("{meta}\n{usage}")).unwrap();
        }

        let providers: Vec<String> = PROVIDERS.iter().map(|p| p.to_string()).collect();
        let window = Window { cwds: None, start_ms: start, end_ms: now };
        let rows = UsageReader::default().measure(home.path(), &providers, &window);

        let claude = rows.iter().find(|row| row.provider == "claude").unwrap();
        assert_eq!((claude.sessions, claude.input_tokens), (2, 20), "the session from before the window is left out");
        let codex = rows.iter().find(|row| row.provider == "codex").unwrap();
        assert_eq!((codex.sessions, codex.input_tokens, codex.output_tokens), (2, 10, 4));
        assert!(!rows.iter().find(|row| row.provider == "opencode").unwrap().measured);
    }

    #[test]
    fn a_session_is_read_again_only_after_its_file_changes() {
        let home = tempfile::tempdir().unwrap();
        let repo = home.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        let project = home.path().join(".claude/projects").join(claude_project_dir_name(&canonical_string(&repo)));
        fs::create_dir_all(&project).unwrap();
        let now = now_ms();
        let line = |request: &str, input: u64| {
            format!(
                r#"{{"type":"assistant","requestId":"{request}","cwd":"{}","timestamp":"{}","message":{{"usage":{{"input_tokens":{input},"output_tokens":0}}}}}}"#,
                repo.display(),
                format_iso(now - 60_000)
            )
        };
        let path = project.join("live.jsonl");
        fs::write(&path, line("r1", 100)).unwrap();

        let providers = vec!["claude".to_string()];
        let window = Window { cwds: Some(vec![repo.clone()]), start_ms: now - 120_000, end_ms: now + 60_000 };
        let reader = UsageReader::default();
        assert_eq!(reader.measure(home.path(), &providers, &window)[0].input_tokens, 100);

        // The reader keeps the parse for an unchanged file, and notices a file that grew.
        assert_eq!(reader.measure(home.path(), &providers, &window)[0].input_tokens, 100);
        let mut grown = fs::read_to_string(&path).unwrap();
        grown.push('\n');
        grown.push_str(&line("r2", 40));
        fs::write(&path, grown).unwrap();
        assert_eq!(reader.measure(home.path(), &providers, &window)[0].input_tokens, 140);
    }
}
