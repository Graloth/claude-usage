//! Aggregates Claude Code transcripts (`<config dir>/projects/**/*.jsonl`)
//! into the numbers the popup shows: API-equivalent cost, token totals, a
//! per-day series and a weekday-by-hour heatmap for the last N days.
//!
//! Transcripts grow into hundreds of megabytes, so files are streamed line by
//! line and, when a cache path is given, each file's parsed records are kept
//! in a JSON cache and only the bytes appended since the last scan are read.
//!
//! Port of the original extension's `scanner.js`; the semantics (dedupe,
//! cache-write split, logged cost, streak) are kept, with these deliberate
//! differences: the cache is discarded when a scan asks for a wider window
//! than the one that produced it, and symlinks below `projects/` are not
//! followed.

use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, BufRead, BufReader, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use jiff::{Span, Timestamp, civil::Date, tz::TimeZone};
use serde::{Deserialize, Serialize};

use crate::pricing::{TokenUsage, cost_of, family_of};

/// Bumped whenever the cache layout or the meaning of a record changes.
const CACHE_VERSION: u32 = 1;
/// Upper bound on `days`, so a bad value cannot allocate without limit.
const MAX_DAYS: u32 = 3660;
/// Directory depth below `projects/` that is still searched.
const MAX_DEPTH: u32 = 32;
const READ_BUFFER: usize = 256 * 1024;
/// A line buffer that grew past this (a transcript line holding a huge
/// attachment) is released after use instead of being kept for the whole scan.
const MAX_KEPT_LINE: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Window length including today. Clamped to `1..=3660`.
    pub days: u32,
    /// Claude config directories; each is searched under `projects/`.
    pub config_dirs: Vec<PathBuf>,
    /// Where to keep the incremental cache. `None` disables caching.
    pub cache_path: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("could not compute the scan window: {0}")]
    Window(String),
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Totals {
    pub cost: f64,
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
    pub messages: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FamilyCost {
    pub name: String,
    pub cost: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DayStat {
    /// Local date, `YYYY-MM-DD`.
    pub date: String,
    pub cost: f64,
    /// Input + output + cache writes. Cache reads dominate raw counts and say
    /// little about activity, so they are left out.
    pub tokens: u64,
    pub messages: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct HourStat {
    pub cost: f64,
    pub tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub generated_at_ms: i64,
    pub days: u32,
    pub totals: Totals,
    /// Cost per model family, most expensive first.
    pub families: Vec<FamilyCost>,
    /// Oldest first, `days` entries, the last one is today.
    pub daily: Vec<DayStat>,
    /// 168 entries indexed `weekday * 24 + hour` in local time, Monday first.
    pub hours: Vec<HourStat>,
    pub active_days: u32,
    /// Consecutive active days up to today. A today without usage yet does
    /// not break a streak that ran through yesterday.
    pub streak: u32,
    pub files_scanned: usize,
    /// Files or cache problems that were skipped over.
    pub warnings: Vec<String>,
}

/// One logged API response: `[dedupe key, timestamp ms, model, input, output,
/// 5m cache write, 1h cache write, cache read, fast (0/1), logged cost USD]`.
/// A tuple struct so the cache stores a compact array per record. The logged
/// cost is only present for older Claude Code versions and API-key users.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Record(
    String,
    i64,
    String,
    u64,
    u64,
    u64,
    u64,
    u64,
    u8,
    Option<f64>,
);

/// What the cache remembers about one transcript file.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileEntry {
    mtime_ms: i64,
    size: u64,
    /// Bytes consumed so far: up to and including the last newline seen.
    offset: u64,
    records: Vec<Record>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Cache {
    version: u32,
    /// Start of the window the records were collected for. Records older than
    /// this were dropped at parse time, so the cache cannot serve a scan that
    /// starts earlier.
    since_ms: i64,
    files: HashMap<String, FileEntry>,
}

impl Cache {
    fn empty(since_ms: i64) -> Self {
        Self {
            version: CACHE_VERSION,
            since_ms,
            files: HashMap::new(),
        }
    }
}

/// Where the cache lives by default:
/// - macOS: `~/Library/Caches/claude-usage/scan.json`
/// - Windows: `%LOCALAPPDATA%\claude-usage\scan.json`
/// - elsewhere: `$XDG_CACHE_HOME` or `~/.cache`, then `claude-usage/scan.json`
pub fn default_cache_path() -> Option<PathBuf> {
    let var = |name| std::env::var_os(name).filter(|v| !v.is_empty());
    let base = if cfg!(windows) {
        var("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::home_dir().map(|home| home.join("Library/Caches"))
    } else {
        var("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::home_dir().map(|home| home.join(".cache")))
    };
    base.map(|dir| dir.join("claude-usage").join("scan.json"))
}

/// Scans the transcripts and aggregates the last `opts.days` local days.
///
/// `now` and `tz` are parameters so tests can fix them; real callers pass
/// `Timestamp::now()` and `TimeZone::system()`.
pub fn scan(opts: &ScanOptions, now: Timestamp, tz: &TimeZone) -> Result<ScanReport, ScanError> {
    let days = opts.days.clamp(1, MAX_DAYS);
    let window = Window::new(now, tz, days).map_err(|e| ScanError::Window(e.to_string()))?;
    let mut warnings = Vec::new();

    let mut files = Vec::new();
    for dir in &opts.config_dirs {
        list_transcripts(&dir.join("projects"), 0, &mut files, &mut warnings);
    }

    // Oldest first, ties by path: when the same response is logged in several
    // files (a resumed session) the most recently written file wins, and not
    // whichever one the file system lists last.
    files.sort_by(|a, b| (a.mtime_ms, &a.path).cmp(&(b.mtime_ms, &b.path)));

    let mut cache = opts.cache_path.as_deref().map_or_else(
        || Cache::empty(window.since_ms),
        |path| load_cache(path, window.since_ms),
    );
    let mut next_files = HashMap::new();
    // Streamed responses are logged several times: the last write wins.
    let mut by_key: HashMap<String, Record> = HashMap::new();

    for file in files.iter().filter(|f| f.mtime_ms >= window.since_ms) {
        let key = file.path.to_string_lossy().into_owned();
        let previous = cache.files.remove(&key);
        let mut entry = match previous {
            Some(entry) if entry.mtime_ms == file.mtime_ms && entry.size == file.size => entry,
            previous => match scan_file(file, previous, window.since_ms) {
                Ok(entry) => entry,
                Err(e) => {
                    warnings.push(format!("skipped {}: {e}", file.path.display()));
                    continue;
                }
            },
        };
        entry.records.retain(|r| r.1 >= window.since_ms);
        for record in &entry.records {
            by_key.insert(record.0.clone(), record.clone());
        }
        next_files.insert(key, entry);
    }

    let files_scanned = next_files.len();
    if let Some(path) = &opts.cache_path {
        let cache = Cache {
            version: CACHE_VERSION,
            since_ms: window.since_ms,
            files: next_files,
        };
        if let Err(e) = save_cache(path, &cache) {
            warnings.push(format!("could not write cache {}: {e}", path.display()));
        }
    }

    // Sorted so float sums, and so reports, do not depend on hash order.
    let mut records: Vec<Record> = by_key.into_values().collect();
    records.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));

    let mut report = aggregate(&records, &window, tz, now);
    report.files_scanned = files_scanned;
    report.warnings = warnings;
    Ok(report)
}

/// [`scan`] for the current moment in the system time zone (UTC when the zone
/// cannot be detected), so callers need no `jiff` dependency of their own.
pub fn scan_local(opts: &ScanOptions) -> Result<ScanReport, ScanError> {
    scan(opts, Timestamp::now(), &TimeZone::system())
}

/// The days a scan covers, in local time.
struct Window {
    days: u32,
    since_ms: i64,
    first_date: Date,
}

impl Window {
    fn new(now: Timestamp, tz: &TimeZone, days: u32) -> Result<Self, jiff::Error> {
        let today = now.to_zoned(tz.clone()).date();
        let first_date = today.checked_sub(Span::new().days(i64::from(days) - 1))?;
        // Local midnight; `to_zoned` resolves a midnight skipped by DST.
        let since = first_date.to_zoned(tz.clone())?.timestamp();
        Ok(Self {
            days,
            since_ms: since.as_millisecond(),
            first_date,
        })
    }
}

struct TranscriptFile {
    path: PathBuf,
    mtime_ms: i64,
    size: u64,
}

/// Recursively collects `.jsonl` files. Unreadable directories are reported,
/// not fatal; a missing `projects/` just means no transcripts.
fn list_transcripts(
    dir: &Path,
    depth: u32,
    out: &mut Vec<TranscriptFile>,
    warnings: &mut Vec<String>,
) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => {
            warnings.push(format!("could not read {}: {e}", dir.display()));
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // `file_type` does not follow symlinks, which rules out loops.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth < MAX_DEPTH {
                list_transcripts(&path, depth + 1, out, warnings);
            }
        } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
            match entry.metadata().and_then(|m| Ok((m.len(), m.modified()?))) {
                Ok((size, modified)) => out.push(TranscriptFile {
                    path,
                    mtime_ms: system_time_ms(modified),
                    size,
                }),
                Err(e) => warnings.push(format!("could not stat {}: {e}", path.display())),
            }
        }
    }
}

fn system_time_ms(time: std::time::SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

/// Parses the part of `file` not yet in `previous`.
fn scan_file(
    file: &TranscriptFile,
    previous: Option<FileEntry>,
    since_ms: i64,
) -> io::Result<FileEntry> {
    // Re-read from scratch when the file was truncated or rewritten.
    let (start, mut records) = match previous {
        Some(prev) if file.size >= prev.size && prev.offset <= file.size => {
            (prev.offset, prev.records)
        }
        _ => (0, Vec::new()),
    };

    let mut reader = BufReader::with_capacity(READ_BUFFER, File::open(&file.path)?);
    reader.seek(SeekFrom::Start(start))?;

    let mut consumed = 0_u64;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        // A last line without its newline may still be being written: leave
        // it for the next scan rather than parse half of it.
        if line.last() != Some(&b'\n') {
            break;
        }
        consumed += read as u64;
        if let Some(record) = parse_line(&line, since_ms) {
            records.push(record);
        }
        if line.capacity() > MAX_KEPT_LINE {
            line = Vec::new();
        }
    }

    Ok(FileEntry {
        mtime_ms: file.mtime_ms,
        size: file.size,
        offset: start + consumed,
        records,
    })
}

// Only the fields the scanner needs; serde skips everything else, including
// the large message contents.
#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    uuid: Option<String>,
    /// Kept as a value so a non-numeric `costUSD` is ignored, not fatal.
    #[serde(rename = "costUSD")]
    cost_usd: Option<serde_json::Value>,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation: Option<CacheCreation>,
    speed: Option<String>,
}

#[derive(Deserialize)]
struct CacheCreation {
    ephemeral_1h_input_tokens: Option<u64>,
}

const USAGE_MARKER: &[u8] = b"\"usage\"";

/// One transcript line as a record, if it is an assistant response with
/// usage inside the window. Anything else, including malformed lines, is
/// skipped silently.
fn parse_line(line: &[u8], since_ms: i64) -> Option<Record> {
    // Cheap pre-filter: most lines are not usage-bearing.
    if !line.windows(USAGE_MARKER.len()).any(|w| w == USAGE_MARKER) {
        return None;
    }
    let parsed: Line = serde_json::from_slice(line).ok()?;
    let message = parsed.message?;
    let usage = message.usage?;
    let model = message
        .model
        .filter(|m| !m.is_empty() && !m.starts_with('<'))?;
    if parsed.kind.as_deref() != Some("assistant") {
        return None;
    }
    let ts_ms = parsed
        .timestamp?
        .parse::<Timestamp>()
        .ok()?
        .as_millisecond();
    if ts_ms < since_ms {
        return None;
    }

    let write_total = usage.cache_creation_input_tokens.unwrap_or(0);
    let write_1h = usage
        .cache_creation
        .and_then(|c| c.ephemeral_1h_input_tokens)
        .unwrap_or(0);
    let discriminator = parsed
        .request_id
        .or(parsed.uuid)
        .unwrap_or_else(|| ts_ms.to_string());

    Some(Record(
        format!("{}:{discriminator}", message.id.as_deref().unwrap_or("")),
        ts_ms,
        model,
        usage.input_tokens.unwrap_or(0),
        usage.output_tokens.unwrap_or(0),
        write_total.saturating_sub(write_1h),
        write_1h,
        usage.cache_read_input_tokens.unwrap_or(0),
        u8::from(usage.speed.as_deref() == Some("fast")),
        parsed.cost_usd.and_then(|v| v.as_f64()),
    ))
}

/// A cache that is missing, unreadable, from another version or narrower than
/// the requested window is simply replaced by an empty one.
fn load_cache(path: &Path, since_ms: i64) -> Cache {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Cache>(&bytes).ok())
        .filter(|cache| cache.version == CACHE_VERSION && cache.since_ms <= since_ms)
        .unwrap_or_else(|| Cache::empty(since_ms))
}

/// Writes through a temporary file so a crash or a concurrent reader never
/// sees half a cache.
fn save_cache(path: &Path, cache: &Cache) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let result = (|| {
        let mut writer = io::BufWriter::new(File::create(&tmp)?);
        serde_json::to_writer(&mut writer, cache).map_err(io::Error::from)?;
        writer.flush()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

fn aggregate(records: &[Record], window: &Window, tz: &TimeZone, now: Timestamp) -> ScanReport {
    let mut daily: Vec<DayStat> = Vec::new();
    let mut day_index: HashMap<Date, usize> = HashMap::new();
    let mut date = window.first_date;
    for i in 0..window.days as usize {
        day_index.insert(date, i);
        daily.push(DayStat {
            date: date.to_string(),
            cost: 0.0,
            tokens: 0,
            messages: 0,
        });
        // Past the last representable date the series simply stops growing.
        match date.checked_add(Span::new().days(1)) {
            Ok(next) => date = next,
            Err(_) => break,
        }
    }

    let mut hours = vec![HourStat::default(); 7 * 24];
    let mut totals = Totals::default();
    let mut families: HashMap<&'static str, f64> = HashMap::new();

    for Record(_, ts_ms, model, input, output, write_5m, write_1h, read, fast, logged) in records {
        let cost = logged.unwrap_or_else(|| {
            cost_of(
                model,
                &TokenUsage {
                    input: *input,
                    output: *output,
                    write_5m: *write_5m,
                    write_1h: *write_1h,
                    read: *read,
                    fast: *fast != 0,
                },
            )
        });
        let tokens = input + output + write_5m + write_1h;

        totals.cost += cost;
        totals.input += input;
        totals.output += output;
        totals.cache_write += write_5m + write_1h;
        totals.cache_read += read;
        totals.messages += 1;
        *families.entry(family_of(model)).or_default() += cost;

        let Ok(at) = Timestamp::from_millisecond(*ts_ms).map(|t| t.to_zoned(tz.clone())) else {
            continue;
        };
        if let Some(&i) = day_index.get(&at.date()) {
            let day = &mut daily[i];
            day.cost += cost;
            day.tokens += tokens;
            day.messages += 1;
        }
        let slot =
            &mut hours[at.weekday().to_monday_zero_offset() as usize * 24 + at.hour() as usize];
        slot.cost += cost;
        slot.tokens += tokens;
    }

    let mut families: Vec<FamilyCost> = families
        .into_iter()
        .map(|(name, cost)| FamilyCost {
            name: name.to_owned(),
            cost,
        })
        .collect();
    families.sort_by(|a, b| b.cost.total_cmp(&a.cost).then_with(|| a.name.cmp(&b.name)));

    ScanReport {
        generated_at_ms: now.as_millisecond(),
        days: window.days,
        totals,
        families,
        active_days: daily.iter().filter(|d| d.messages > 0).count() as u32,
        streak: streak(&daily),
        daily,
        hours,
        files_scanned: 0,
        warnings: Vec::new(),
    }
}

/// Consecutive active days ending today. Today without usage yet does not
/// break a streak that ran through yesterday.
fn streak(daily: &[DayStat]) -> u32 {
    let mut streak = 0;
    for (i, day) in daily.iter().enumerate().rev() {
        if day.messages > 0 {
            streak += 1;
        } else if i != daily.len() - 1 {
            break;
        }
    }
    streak
}

#[cfg(test)]
mod tests {
    use std::{str::FromStr, time::SystemTime};

    use tempfile::TempDir;

    use super::*;

    const MODEL: &str = "claude-sonnet-4-5"; // $3 in, $15 out per million

    fn copenhagen() -> TimeZone {
        TimeZone::get("Europe/Copenhagen").expect("tz database available")
    }

    fn ts(s: &str) -> Timestamp {
        Timestamp::from_str(s).unwrap()
    }

    /// A fake config directory with one project holding transcript files.
    struct Fixture {
        dir: TempDir,
        now: Timestamp,
    }

    impl Fixture {
        fn new(now: &str) -> Self {
            Self {
                dir: TempDir::new().unwrap(),
                now: ts(now),
            }
        }

        fn project(&self) -> PathBuf {
            let dir = self.dir.path().join("projects").join("demo");
            fs::create_dir_all(&dir).unwrap();
            dir
        }

        /// Writes (replaces) a transcript and stamps its mtime as `now`, so the
        /// scan window never skips it.
        fn write(&self, name: &str, lines: &[String]) -> PathBuf {
            let path = self.project().join(name);
            let mut text = lines.join("\n");
            text.push('\n');
            fs::write(&path, text).unwrap();
            self.touch(&path);
            path
        }

        fn append(&self, path: &Path, text: &str) {
            let mut file = fs::OpenOptions::new().append(true).open(path).unwrap();
            file.write_all(text.as_bytes()).unwrap();
            drop(file);
            self.touch(path);
        }

        fn touch(&self, path: &Path) {
            let file = File::options().write(true).open(path).unwrap();
            file.set_modified(SystemTime::from(self.now)).unwrap();
        }

        fn cache(&self) -> PathBuf {
            self.dir.path().join("cache").join("scan.json")
        }

        fn options(&self, days: u32) -> ScanOptions {
            ScanOptions {
                days,
                config_dirs: vec![self.dir.path().to_path_buf()],
                cache_path: Some(self.cache()),
            }
        }

        fn scan(&self, days: u32) -> ScanReport {
            scan(&self.options(days), self.now, &copenhagen()).unwrap()
        }
    }

    /// An assistant line with 1M input and 0 output tokens unless overridden.
    fn line(id: &str, request: &str, at: &str, model: &str, input: u64, output: u64) -> String {
        serde_json::json!({
            "type": "assistant",
            "timestamp": at,
            "requestId": request,
            "message": {
                "id": id,
                "model": model,
                "usage": {"input_tokens": input, "output_tokens": output},
                "content": [{"type": "text", "text": "hello"}],
            },
        })
        .to_string()
    }

    fn sonnet(id: &str, at: &str, input: u64, output: u64) -> String {
        line(id, &format!("req-{id}"), at, MODEL, input, output)
    }

    fn day<'a>(report: &'a ScanReport, date: &str) -> &'a DayStat {
        report.daily.iter().find(|d| d.date == date).unwrap()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn totals_cost_and_token_metric() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        let record = serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-10-19T08:00:00Z",
            "requestId": "r1",
            "message": {"id": "m1", "model": MODEL, "usage": {
                "input_tokens": 1_000_000,
                "output_tokens": 1_000_000,
                // 600k cache writes, 400k of them 1-hour.
                "cache_creation_input_tokens": 600_000,
                "cache_creation": {"ephemeral_1h_input_tokens": 400_000},
                "cache_read_input_tokens": 2_000_000,
            }},
        });
        fx.write("a.jsonl", &[record.to_string()]);

        let report = fx.scan(7);
        // 3 + 15 + 200k*3.75 + 400k*6 + 2M*0.3 = 3+15+0.75+2.4+0.6
        assert!(close(report.totals.cost, 21.75), "{}", report.totals.cost);
        assert_eq!(report.totals.input, 1_000_000);
        assert_eq!(report.totals.output, 1_000_000);
        assert_eq!(report.totals.cache_write, 600_000);
        assert_eq!(report.totals.cache_read, 2_000_000);
        assert_eq!(report.totals.messages, 1);
        // The activity metric leaves cache reads out.
        assert_eq!(day(&report, "2026-10-19").tokens, 2_600_000);
        assert_eq!(report.families.len(), 1);
        assert_eq!(report.families[0].name, "Sonnet");
        assert_eq!(report.files_scanned, 1);
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn streamed_duplicates_count_once_and_the_last_write_wins() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        let at = "2026-10-19T08:00:00Z";
        fx.write(
            "a.jsonl",
            &[
                line("m1", "r1", at, MODEL, 10, 5),
                line("m1", "r1", at, MODEL, 10, 50), // same response, more output
                line("m2", "r2", at, MODEL, 10, 1),
            ],
        );
        // The same response also logged in a second file (resumed session).
        fx.write("b.jsonl", &[line("m2", "r2", at, MODEL, 10, 7)]);

        let report = fx.scan(7);
        assert_eq!(report.totals.messages, 2);
        assert_eq!(report.totals.output, 50 + 7);
    }

    #[test]
    fn dedupe_key_falls_back_to_uuid_then_timestamp() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        let at = "2026-10-19T08:00:00Z";
        let no_request = |uuid: Option<&str>, output: u64| {
            serde_json::json!({
                "type": "assistant", "timestamp": at, "uuid": uuid,
                "message": {"id": "m", "model": MODEL,
                    "usage": {"input_tokens": 1, "output_tokens": output}},
            })
            .to_string()
        };
        fx.write(
            "a.jsonl",
            &[
                no_request(Some("u1"), 1),
                no_request(Some("u1"), 2), // same uuid: duplicate
                no_request(Some("u2"), 3),
                no_request(None, 4),
                no_request(None, 5), // same timestamp: duplicate
            ],
        );
        let report = fx.scan(7);
        assert_eq!(report.totals.messages, 3);
        assert_eq!(report.totals.output, 2 + 3 + 5);
    }

    #[test]
    fn irrelevant_and_malformed_lines_are_skipped() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        let at = "2026-10-19T08:00:00Z";
        let mut user = serde_json::from_str::<serde_json::Value>(&sonnet("u", at, 1, 1)).unwrap();
        user["type"] = "user".into();
        fx.write(
            "a.jsonl",
            &[
                sonnet("ok", at, 100, 0),
                user.to_string(),
                line("syn", "r", at, "<synthetic>", 100, 0),
                line("nomodel", "r", at, "", 100, 0),
                "not json but mentions \"usage\"".to_owned(),
                r#"{"type":"summary","note":"no tokens here"}"#.to_owned(),
                r#"{"type":"assistant","timestamp":"garbage","message":{"id":"x","model":"claude-sonnet-4-5","usage":{"input_tokens":1}}}"#.to_owned(),
                sonnet("old", "2026-09-01T08:00:00Z", 100, 0),
            ],
        );
        let report = fx.scan(7);
        assert_eq!(report.totals.messages, 1);
        assert_eq!(report.totals.input, 100);
    }

    #[test]
    fn logged_cost_and_fast_mode() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        let at = "2026-10-19T08:00:00Z";
        let mut logged =
            serde_json::from_str::<serde_json::Value>(&sonnet("a", at, 1_000_000, 0)).unwrap();
        logged["costUSD"] = 0.42.into();
        let mut junk_cost =
            serde_json::from_str::<serde_json::Value>(&sonnet("b", at, 1_000_000, 0)).unwrap();
        junk_cost["costUSD"] = "free".into();
        let mut fast = serde_json::from_str::<serde_json::Value>(&line(
            "c",
            "rc",
            at,
            "claude-opus-5-5",
            1_000_000,
            0,
        ))
        .unwrap();
        fast["message"]["usage"]["speed"] = "fast".into();
        fx.write(
            "a.jsonl",
            &[logged.to_string(), junk_cost.to_string(), fast.to_string()],
        );

        let report = fx.scan(7);
        // 0.42 logged + $3 computed (junk cost ignored) + $8 fast Opus 5.5.
        assert!(
            close(report.totals.cost, 0.42 + 3.0 + 8.0),
            "{}",
            report.totals.cost
        );
    }

    #[test]
    fn days_are_local_oldest_first_and_include_today() {
        let fx = Fixture::new("2026-10-19T12:00:00Z"); // Monday, CEST (+02:00)
        fx.write(
            "a.jsonl",
            &[
                // 23:30 UTC on the 17th is already the 18th in Copenhagen.
                sonnet("a", "2026-10-17T23:30:00Z", 1_000_000, 0),
                sonnet("b", "2026-10-19T08:00:00Z", 1_000_000, 0),
            ],
        );
        let report = fx.scan(3);
        let dates: Vec<_> = report.daily.iter().map(|d| d.date.as_str()).collect();
        assert_eq!(dates, ["2026-10-17", "2026-10-18", "2026-10-19"]);
        assert_eq!(report.daily[0].messages, 0);
        assert_eq!(report.daily[1].messages, 1);
        assert!(close(report.daily[1].cost, 3.0));
        assert_eq!(report.daily[2].messages, 1);
        assert_eq!(report.active_days, 2);
        assert_eq!(report.streak, 2);
    }

    #[test]
    fn hours_are_monday_first_in_local_time() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        fx.write(
            "a.jsonl",
            &[
                // Monday 08:15 in Copenhagen (06:15 UTC).
                sonnet("a", "2026-10-19T06:15:00Z", 1_000_000, 0),
                // Sunday 18 Oct 23:59 local.
                sonnet("b", "2026-10-18T21:59:00Z", 1_000_000, 0),
            ],
        );
        let report = fx.scan(7);
        assert_eq!(report.hours.len(), 168);
        assert!(close(report.hours[8].cost, 3.0)); // Monday = 0, hour 8
        assert!(close(report.hours[6 * 24 + 23].cost, 3.0)); // Sunday = 6
        assert_eq!(report.hours.iter().filter(|h| h.cost > 0.0).count(), 2);
    }

    #[test]
    fn a_dst_change_day_has_one_bucket_and_its_repeated_hour_is_shared() {
        // Copenhagen leaves summer time on Sunday 2026-10-25: 03:00 CEST
        // becomes 02:00 CET, so the day has 25 hours and 02:xx happens twice.
        let fx = Fixture::new("2026-10-26T10:00:00+01:00");
        fx.write(
            "a.jsonl",
            &[
                sonnet("a", "2026-10-25T00:30:00+02:00", 1_000_000, 0),
                sonnet("b", "2026-10-25T02:30:00+02:00", 1_000_000, 0), // first 02:30
                sonnet("c", "2026-10-25T02:30:00+01:00", 1_000_000, 0), // second 02:30
                sonnet("d", "2026-10-25T23:30:00+01:00", 1_000_000, 0),
                sonnet("e", "2026-10-26T00:10:00+01:00", 1_000_000, 0),
            ],
        );
        let report = fx.scan(3);
        assert_eq!(report.daily.len(), 3);
        assert_eq!(report.daily[0].date, "2026-10-24");
        assert_eq!(day(&report, "2026-10-25").messages, 4);
        assert_eq!(day(&report, "2026-10-26").messages, 1);
        // Sunday = 6; both 02:30s land in the same hour slot.
        assert!(close(report.hours[6 * 24 + 2].cost, 6.0));
        assert!(close(report.hours[6 * 24].cost, 3.0));
        assert!(close(report.hours[6 * 24 + 23].cost, 3.0));
    }

    #[test]
    fn the_window_starts_at_local_midnight_even_across_a_dst_change() {
        // 3 days back from 26 Oct spans the 25-hour day.
        let window = Window::new(ts("2026-10-26T10:00:00+01:00"), &copenhagen(), 3).unwrap();
        assert_eq!(window.first_date.to_string(), "2026-10-24");
        assert_eq!(
            Timestamp::from_millisecond(window.since_ms).unwrap(),
            ts("2026-10-24T00:00:00+02:00")
        );
    }

    #[test]
    fn streak_edge_cases() {
        let days = |active: &[bool]| -> Vec<DayStat> {
            active
                .iter()
                .enumerate()
                .map(|(i, &on)| DayStat {
                    date: format!("d{i}"),
                    cost: 0.0,
                    tokens: 0,
                    messages: u64::from(on),
                })
                .collect()
        };
        assert_eq!(streak(&days(&[true, true, true])), 3);
        // Today empty, yesterday and before active: the streak survives.
        assert_eq!(streak(&days(&[true, true, false])), 2);
        // Today and yesterday empty: over.
        assert_eq!(streak(&days(&[true, true, false, false])), 0);
        // A gap in the middle ends it.
        assert_eq!(streak(&days(&[true, false, true, true])), 2);
        assert_eq!(streak(&days(&[false])), 0);
        assert_eq!(streak(&days(&[true])), 1);
        assert_eq!(streak(&[]), 0);
    }

    #[test]
    fn families_are_sorted_by_cost() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        let at = "2026-10-19T08:00:00Z";
        fx.write(
            "a.jsonl",
            &[
                line("a", "r1", at, "claude-haiku-4-5", 1_000_000, 0), // $1
                line("b", "r2", at, "claude-opus-4-5", 1_000_000, 0),  // $5
                line("c", "r3", at, MODEL, 1_000_000, 0),              // $3
                line("d", "r4", at, "some-local-model", 1_000_000, 0), // free
            ],
        );
        let report = fx.scan(7);
        let names: Vec<_> = report.families.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["Opus", "Sonnet", "Haiku", "Other"]);
    }

    #[test]
    fn files_older_than_the_window_are_not_read() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        let path = fx.write("old.jsonl", &[sonnet("a", "2026-10-19T08:00:00Z", 100, 0)]);
        let file = File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::from(ts("2026-09-01T00:00:00Z")))
            .unwrap();
        let report = fx.scan(7);
        assert_eq!(report.totals.messages, 0);
        assert_eq!(report.files_scanned, 0);
    }

    #[test]
    fn nested_projects_and_other_extensions() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        let deep = fx.project().join("sub").join("agents");
        fs::create_dir_all(&deep).unwrap();
        let record = sonnet("a", "2026-10-19T08:00:00Z", 100, 0);
        fs::write(deep.join("x.jsonl"), format!("{record}\n")).unwrap();
        fs::write(deep.join("x.txt"), format!("{record}\n")).unwrap();
        fx.touch(&deep.join("x.jsonl"));
        let report = fx.scan(7);
        assert_eq!(report.totals.messages, 1);
        assert_eq!(report.files_scanned, 1);
    }

    #[test]
    fn missing_projects_dir_is_an_empty_report_not_an_error() {
        let dir = TempDir::new().unwrap();
        let report = scan(
            &ScanOptions {
                days: 7,
                config_dirs: vec![dir.path().to_path_buf()],
                cache_path: None,
            },
            ts("2026-10-19T12:00:00Z"),
            &copenhagen(),
        )
        .unwrap();
        assert_eq!(report.totals.messages, 0);
        assert_eq!(report.daily.len(), 7);
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn scan_local_covers_today_in_the_system_zone() {
        let dir = TempDir::new().unwrap();
        let report = scan_local(&ScanOptions {
            days: 2,
            config_dirs: vec![dir.path().to_path_buf()],
            cache_path: None,
        })
        .unwrap();
        assert_eq!(report.daily.len(), 2);
        assert_eq!(report.totals.messages, 0);
    }

    #[test]
    fn days_is_clamped() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        assert_eq!(fx.scan(0).daily.len(), 1);
        assert_eq!(fx.scan(u32::MAX).daily.len(), MAX_DAYS as usize);
    }

    #[test]
    fn a_half_written_last_line_is_retried_on_the_next_scan() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        let first = sonnet("a", "2026-10-19T08:00:00Z", 100, 0);
        let second = sonnet("b", "2026-10-19T09:00:00Z", 200, 0);
        let (head, tail) = second.split_at(second.len() / 2);
        let path = fx.write("a.jsonl", &[first]);
        fx.append(&path, head); // no newline yet

        assert_eq!(fx.scan(7).totals.messages, 1);

        fx.append(&path, &format!("{tail}\n"));
        let report = fx.scan(7);
        assert_eq!(report.totals.messages, 2);
        assert_eq!(report.totals.input, 300);
    }

    #[test]
    fn appended_lines_are_read_incrementally() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        let path = fx.write("a.jsonl", &[sonnet("a", "2026-10-19T08:00:00Z", 100, 0)]);
        assert_eq!(fx.scan(7).totals.messages, 1);

        let size_after_first = fs::metadata(&path).unwrap().len();
        let cached: Cache = serde_json::from_slice(&fs::read(fx.cache()).unwrap()).unwrap();
        let entry = cached.files.values().next().unwrap();
        assert_eq!(entry.offset, size_after_first);
        assert_eq!(entry.records.len(), 1);

        fx.append(
            &path,
            &format!("{}\n", sonnet("b", "2026-10-19T09:00:00Z", 50, 0)),
        );
        let report = fx.scan(7);
        assert_eq!(report.totals.messages, 2);
        assert_eq!(report.totals.input, 150);
    }

    #[test]
    fn an_unchanged_file_is_served_from_the_cache() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        let path = fx.write("a.jsonl", &[sonnet("a", "2026-10-19T08:00:00Z", 100, 0)]);
        let first = fx.scan(7);

        // Same size and mtime but different bytes: only a cache hit can still
        // report the old content.
        let swapped = fs::read_to_string(&path).unwrap().replace("100", "999");
        assert_eq!(swapped.len(), fs::metadata(&path).unwrap().len() as usize);
        fs::write(&path, swapped).unwrap();
        fx.touch(&path);

        let second = fx.scan(7);
        assert_eq!(second.totals.input, first.totals.input);
        assert_eq!(second.totals.input, 100);
    }

    #[test]
    fn a_truncated_or_rewritten_file_is_read_from_the_start() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        let path = fx.write(
            "a.jsonl",
            &[
                sonnet("a", "2026-10-19T08:00:00Z", 100, 0),
                sonnet("b", "2026-10-19T08:05:00Z", 100, 0),
            ],
        );
        assert_eq!(fx.scan(7).totals.messages, 2);

        // Replaced by a shorter file: the cached records must not survive.
        fx.write("a.jsonl", &[sonnet("c", "2026-10-19T09:00:00Z", 7, 0)]);
        assert!(path.exists());
        let report = fx.scan(7);
        assert_eq!(report.totals.messages, 1);
        assert_eq!(report.totals.input, 7);
    }

    #[test]
    fn widening_the_window_does_not_reuse_a_narrower_cache() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        fx.write(
            "a.jsonl",
            &[
                sonnet("old", "2026-10-17T08:00:00Z", 100, 0),
                sonnet("new", "2026-10-19T08:00:00Z", 10, 0),
            ],
        );
        assert_eq!(fx.scan(1).totals.messages, 1);
        // The cache now only holds today's record; a 3-day scan needs both.
        let wide = fx.scan(3);
        assert_eq!(wide.totals.messages, 2);
        assert_eq!(wide.totals.input, 110);
    }

    #[test]
    fn narrowing_the_window_reuses_the_cache_and_trims_old_records() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        fx.write(
            "a.jsonl",
            &[
                sonnet("old", "2026-10-17T08:00:00Z", 100, 0),
                sonnet("new", "2026-10-19T08:00:00Z", 10, 0),
            ],
        );
        assert_eq!(fx.scan(3).totals.messages, 2);
        let narrow = fx.scan(1);
        assert_eq!(narrow.totals.messages, 1);
        assert_eq!(narrow.totals.input, 10);
    }

    #[test]
    fn the_cache_round_trips_to_an_identical_report() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        fx.write(
            "a.jsonl",
            &[
                sonnet("a", "2026-10-18T08:00:00Z", 1_000, 10),
                sonnet("b", "2026-10-19T08:00:00Z", 2_000, 20),
            ],
        );
        let cold = fx.scan(7);
        assert!(fx.cache().exists());
        let warm = fx.scan(7);
        assert_eq!(
            serde_json::to_string(&cold).unwrap(),
            serde_json::to_string(&warm).unwrap()
        );
        // And a scan without any cache agrees too.
        let uncached = scan(
            &ScanOptions {
                cache_path: None,
                ..fx.options(7)
            },
            fx.now,
            &copenhagen(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_string(&cold).unwrap(),
            serde_json::to_string(&uncached).unwrap()
        );
    }

    #[test]
    fn a_corrupt_or_old_cache_is_replaced() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        fx.write("a.jsonl", &[sonnet("a", "2026-10-19T08:00:00Z", 100, 0)]);

        fs::create_dir_all(fx.cache().parent().unwrap()).unwrap();
        fs::write(fx.cache(), "{definitely not a cache").unwrap();
        assert_eq!(fx.scan(7).totals.messages, 1);
        // Now valid again.
        assert!(serde_json::from_slice::<Cache>(&fs::read(fx.cache()).unwrap()).is_ok());

        fs::write(
            fx.cache(),
            r#"{"version":0,"since_ms":0,"files":{"x":{"mtime_ms":1,"size":1,"offset":1,"records":[]}}}"#,
        )
        .unwrap();
        let report = fx.scan(7);
        assert_eq!(report.totals.messages, 1);
        let cached: Cache = serde_json::from_slice(&fs::read(fx.cache()).unwrap()).unwrap();
        assert_eq!(cached.version, CACHE_VERSION);
    }

    #[test]
    fn an_unwritable_cache_is_a_warning_not_a_failure() {
        let fx = Fixture::new("2026-10-19T12:00:00Z");
        fx.write("a.jsonl", &[sonnet("a", "2026-10-19T08:00:00Z", 100, 0)]);
        // A regular file where the cache's parent directory should be.
        let blocker = fx.dir.path().join("blocker");
        fs::write(&blocker, "x").unwrap();
        let report = scan(
            &ScanOptions {
                cache_path: Some(blocker.join("scan.json")),
                ..fx.options(7)
            },
            fx.now,
            &copenhagen(),
        )
        .unwrap();
        assert_eq!(report.totals.messages, 1);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("could not write cache"));
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_is_skipped_with_a_warning() {
        use std::os::unix::fs::PermissionsExt;

        let fx = Fixture::new("2026-10-19T12:00:00Z");
        let good = fx.write("good.jsonl", &[sonnet("a", "2026-10-19T08:00:00Z", 100, 0)]);
        let bad = fx.write("bad.jsonl", &[sonnet("b", "2026-10-19T08:00:00Z", 100, 0)]);
        fs::set_permissions(&bad, fs::Permissions::from_mode(0o000)).unwrap();

        let report = fx.scan(7);
        fs::set_permissions(&bad, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(good.exists());

        if report.warnings.is_empty() {
            // Running as root: permissions do not apply, nothing to assert.
            return;
        }
        assert_eq!(report.totals.messages, 1);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("bad.jsonl"));
    }

    #[test]
    fn default_cache_path_ends_in_the_app_directory() {
        if let Some(path) = default_cache_path() {
            assert!(path.ends_with(Path::new("claude-usage").join("scan.json")));
        }
    }
}
