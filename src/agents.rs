//! Resolves what each running agent is doing from the state it keeps on disk.
//!
//! herdr reads agent state off the screens of panes it owns. mast
//! watches agents running in any terminal, so instead it reads the session
//! files each agent already writes:
//!
//! - claude: `~/.claude/sessions/<pid>.json` carries a live `status`; the
//!   session transcript holds the AI-generated title (the same one claude
//!   puts in the terminal title bar).
//! - codex:  the rollout `~/.codex/sessions/**/rollout-*.jsonl` logs
//!   `task_started` / `task_complete` / `turn_aborted` events.
//! - pi:     `~/.pi/agent/sessions/<cwd>/*.jsonl` logs every message; a turn is
//!   in flight until the assistant stops without asking for a tool.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::process::{self, AgentProc, Kind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// Waiting on a human: a permission prompt or a question.
    Blocked,
    Working,
    Idle,
    Unknown,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Blocked => "needs you",
            Status::Working => "working",
            Status::Idle => "idle",
            Status::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Agent {
    pub kind: Kind,
    pub pid: u32,
    pub cwd: Option<PathBuf>,
    pub title: Option<String>,
    pub status: Status,
    /// When the agent entered its current status.
    pub since: SystemTime,
}

pub fn scan() -> Vec<Agent> {
    let procs = process::scan();
    let home = home();

    let codex_pids: Vec<u32> = procs
        .iter()
        .filter(|p| p.kind != Kind::Claude)
        .map(|p| p.proc.pid)
        .collect();
    let open = process::open_files(&codex_pids);
    // Files an agent holds open are definitely its own; keep the cwd-based
    // fallback from handing them to a neighbour in the same directory.
    let mut claimed: HashSet<PathBuf> = open
        .values()
        .flatten()
        .filter(|f| is_jsonl(f))
        .cloned()
        .collect();

    let mut agents: Vec<Agent> = procs
        .iter()
        .map(|p| {
            let mut agent = Agent {
                kind: p.kind,
                pid: p.proc.pid,
                cwd: p.cwd.clone(),
                title: None,
                status: Status::Unknown,
                since: p.proc.started,
            };
            let files = open.get(&p.proc.pid).map(Vec::as_slice).unwrap_or_default();
            match p.kind {
                Kind::Claude => claude(&mut agent, &home),
                Kind::Codex => codex(&mut agent, p, files, &home, &mut claimed),
                Kind::Pi => pi(&mut agent, p, files, &home, &mut claimed),
            }
            agent
        })
        .collect();

    agents
        .sort_by(|a, b| (a.status, &a.cwd, a.kind, a.pid).cmp(&(b.status, &b.cwd, b.kind, b.pid)));
    agents
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn env_dir(var: &str, default: impl FnOnce() -> PathBuf) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(default)
}

// ---------------------------------------------------------------- claude

fn claude(agent: &mut Agent, home: &Path) {
    let dir = env_dir("CLAUDE_CONFIG_DIR", || home.join(".claude"));
    let Ok(raw) = fs::read(dir.join("sessions").join(format!("{}.json", agent.pid))) else {
        return;
    };
    let Ok(v) = serde_json::from_slice::<Value>(&raw) else {
        return;
    };

    agent.status = match v["status"].as_str() {
        Some("busy") => Status::Working,
        Some("waiting" | "blocked") => Status::Blocked,
        Some("idle") => Status::Idle,
        _ => Status::Unknown,
    };
    if let Some(ms) = v["statusUpdatedAt"]
        .as_u64()
        .or_else(|| v["updatedAt"].as_u64())
    {
        agent.since = UNIX_EPOCH + Duration::from_millis(ms);
    }
    if let Some(cwd) = v["cwd"].as_str() {
        agent.cwd = Some(PathBuf::from(cwd));
    }
    agent.title = v["sessionId"]
        .as_str()
        .and_then(|id| claude_transcript(&dir, agent.cwd.as_deref(), id))
        .and_then(|path| claude_title(&path))
        .or_else(|| {
            v["name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        });
}

/// Transcripts live in `projects/<cwd with non-alphanumerics as '-'>/<id>.jsonl`.
/// The session may have moved since it started (worktrees), so fall back to a search.
fn claude_transcript(dir: &Path, cwd: Option<&Path>, id: &str) -> Option<PathBuf> {
    let projects = dir.join("projects");
    let file = format!("{id}.jsonl");
    if let Some(cwd) = cwd {
        let slug: String = cwd
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let path = projects.join(slug).join(&file);
        if path.exists() {
            return Some(path);
        }
    }
    fs::read_dir(&projects)
        .ok()?
        .flatten()
        .map(|e| e.path().join(&file))
        .find(|p| p.exists())
}

#[derive(Default)]
struct TitleScan {
    offset: u64,
    custom: Option<String>,
    ai: Option<String>,
}

/// Transcripts grow to many megabytes, so remember how far each one has been
/// read and only parse what was appended since.
static TITLES: LazyLock<Mutex<HashMap<PathBuf, TitleScan>>> = LazyLock::new(Default::default);

/// A `/rename` title wins over the AI-generated one.
fn claude_title(path: &Path) -> Option<String> {
    let mut titles = TITLES.lock().ok()?;
    let scan = titles.entry(path.to_owned()).or_default();

    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len < scan.offset {
        *scan = TitleScan::default();
    }
    if len > scan.offset {
        file.seek(SeekFrom::Start(scan.offset)).ok()?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        while let Ok(n) = reader.read_line(&mut line) {
            if n == 0 || !line.ends_with('\n') {
                break; // EOF, or a line still being written
            }
            scan.offset += n as u64;
            if line.contains("-title\"")
                && let Ok(v) = serde_json::from_str::<Value>(&line)
            {
                match v["type"].as_str() {
                    Some("custom-title") => {
                        scan.custom = v["customTitle"].as_str().map(str::to_owned)
                    }
                    Some("ai-title") => scan.ai = v["aiTitle"].as_str().map(str::to_owned),
                    _ => {}
                }
            }
            line.clear();
        }
    }
    scan.custom
        .clone()
        .or_else(|| scan.ai.clone())
        .filter(|t| !t.is_empty())
}

// ----------------------------------------------------------------- codex

fn codex(
    agent: &mut Agent,
    p: &AgentProc,
    open: &[PathBuf],
    home: &Path,
    claimed: &mut HashSet<PathBuf>,
) {
    let dir = env_dir("CODEX_HOME", || home.join(".codex"));
    let sessions = dir.join("sessions");

    let rollout = open
        .iter()
        .find(|f| f.starts_with(&sessions) && is_jsonl(f))
        .cloned()
        .or_else(|| {
            newest_matching(recent_files(&sessions, 3), p, claimed, |path| {
                first_line(path)
                    .and_then(|v| v["payload"]["cwd"].as_str().map(PathBuf::from))
                    .is_some_and(|cwd| Some(&cwd) == p.cwd.as_ref())
            })
        });

    let Some(rollout) = rollout else {
        agent.status = Status::Idle;
        return;
    };
    claimed.insert(rollout.clone());

    agent.status = Status::Idle;
    let mut session_id = None;
    for v in tail_json(&rollout) {
        if v["type"] == "session_meta" {
            session_id = v["payload"]["id"].as_str().map(str::to_owned);
        }
        if v["type"] != "event_msg" {
            continue;
        }
        let status = match v["payload"]["type"].as_str() {
            Some("task_started") => Status::Working,
            Some("task_complete" | "turn_aborted") => Status::Idle,
            _ => continue,
        };
        agent.status = status;
        if let Some(ts) = v["timestamp"].as_str().and_then(parse_rfc3339) {
            agent.since = ts;
        }
    }

    let session_id = session_id.or_else(|| {
        first_line(&rollout)?["payload"]["id"]
            .as_str()
            .map(str::to_owned)
    });
    if let Some(id) = session_id {
        agent.title = codex_thread_name(&dir, &id);
    }
}

fn codex_thread_name(dir: &Path, id: &str) -> Option<String> {
    let file = File::open(dir.join("session_index.jsonl")).ok()?;
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|l| l.contains(id))
        .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
        .filter(|v| v["id"] == id)
        .filter_map(|v| v["thread_name"].as_str().map(str::to_owned))
        .last()
}

// -------------------------------------------------------------------- pi

fn pi(
    agent: &mut Agent,
    p: &AgentProc,
    open: &[PathBuf],
    home: &Path,
    claimed: &mut HashSet<PathBuf>,
) {
    let dir = env_dir("PI_CODING_AGENT_DIR", || home.join(".pi").join("agent"));
    let sessions = dir.join("sessions");

    let session = open
        .iter()
        .find(|f| f.starts_with(&sessions) && is_jsonl(f))
        .cloned()
        .or_else(|| {
            let cwd = p.cwd.as_ref()?;
            let files = list_jsonl(&sessions.join(pi_slug(cwd)));
            newest_matching(files, p, claimed, |_| true)
        });

    agent.status = Status::Idle;
    let Some(session) = session else { return };
    claimed.insert(session.clone());

    for v in tail_json(&session) {
        if v["type"] == "session_info"
            && let Some(name) = v["name"].as_str()
        {
            agent.title = Some(name.to_owned());
        }
        if v["type"] != "message" {
            continue;
        }
        let msg = &v["message"];
        agent.status = match (msg["role"].as_str(), msg["stopReason"].as_str()) {
            (Some("user" | "toolResult"), _) => Status::Working,
            (Some("assistant"), Some("toolUse")) => Status::Working,
            (Some("assistant"), _) => Status::Idle,
            _ => continue,
        };
        if let Some(ts) = v["timestamp"].as_str().and_then(parse_rfc3339) {
            agent.since = ts;
        }
        if agent.title.is_none() && msg["role"] == "user" {
            agent.title = first_text(&msg["content"]);
        }
    }
}

/// pi stores sessions under the cwd with separators flattened: `/a/b` -> `--a-b--`.
fn pi_slug(cwd: &Path) -> String {
    let s = cwd.to_string_lossy();
    let flat: String = s
        .trim_start_matches('/')
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '-'
            } else {
                c
            }
        })
        .collect();
    format!("--{flat}--")
}

fn first_text(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(s) => s.as_str(),
        Value::Array(parts) => parts.iter().find_map(|p| p["text"].as_str())?,
        _ => return None,
    };
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(line.to_owned())
}

// ------------------------------------------------------------ file utils

fn is_jsonl(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "jsonl")
}

fn mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn list_jsonl(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_jsonl(p))
        .collect()
}

/// Rollouts live in `sessions/YYYY/MM/DD/`; only look at the most recent days.
fn recent_files(sessions: &Path, days: usize) -> Vec<PathBuf> {
    fn sorted_dirs(dir: &Path) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort_unstable_by(|a, b| b.cmp(a));
        dirs
    }
    sorted_dirs(sessions)
        .iter()
        .flat_map(|y| sorted_dirs(y))
        .flat_map(|m| sorted_dirs(&m))
        .take(days)
        .flat_map(|d| list_jsonl(&d))
        .collect()
}

/// Newest unclaimed session file written since the process started.
fn newest_matching(
    files: Vec<PathBuf>,
    p: &AgentProc,
    claimed: &HashSet<PathBuf>,
    accept: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let floor = p
        .proc
        .started
        .checked_sub(Duration::from_secs(5))
        .unwrap_or(UNIX_EPOCH);
    let mut files: Vec<(SystemTime, PathBuf)> = files
        .into_iter()
        .filter(|f| !claimed.contains(f))
        .filter_map(|f| Some((mtime(&f)?, f)))
        .filter(|(t, _)| *t >= floor)
        .collect();
    files.sort_unstable_by_key(|f| std::cmp::Reverse(f.0));
    files.into_iter().map(|(_, f)| f).find(|f| accept(f))
}

fn first_line(path: &Path) -> Option<Value> {
    let mut line = String::new();
    BufReader::new(File::open(path).ok()?)
        .read_line(&mut line)
        .ok()?;
    serde_json::from_str(&line).ok()
}

/// Parsed JSON lines from the last chunk of a log, oldest first.
fn tail_json(path: &Path) -> Vec<Value> {
    const TAIL: u64 = 512 * 1024;
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(TAIL);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines = text.lines();
    if start > 0 {
        lines.next(); // likely cut mid-line
    }
    lines.filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// `YYYY-MM-DDTHH:MM:SS[.fff]Z`, the only shape these logs write.
fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    let s = s.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let (hms, frac) = time.split_once('.').unwrap_or((time, ""));
    let mut t = hms.split(':').map(|x| x.parse::<i64>());
    let (hh, mm, ss) = (t.next()?.ok()?, t.next()?.ok()?, t.next()?.ok()?);
    let nanos: u32 = format!("{frac:0<9}").get(..9)?.parse().ok()?;

    // Howard Hinnant's days_from_civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;

    let secs = days * 86_400 + hh * 3600 + mm * 60 + ss;
    Some(UNIX_EPOCH + Duration::new(u64::try_from(secs).ok()?, nanos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_timestamps() {
        let t = parse_rfc3339("2026-10-13T05:03:19.834Z").unwrap();
        let ms = t.duration_since(UNIX_EPOCH).unwrap().as_millis();
        assert_eq!(ms, 1_791_867_799_834);
        assert!(parse_rfc3339("1970-01-01T00:00:00Z").is_some());
        assert!(parse_rfc3339("garbage").is_none());
    }

    #[test]
    fn slugs_cwd_like_pi() {
        assert_eq!(
            pi_slug(Path::new("/Users/me/Downloads")),
            "--Users-me-Downloads--"
        );
    }

    #[test]
    fn picks_first_text() {
        let v: Value = serde_json::json!([{"type": "text", "text": "\n  fix the bug\nplease"}]);
        assert_eq!(first_text(&v).as_deref(), Some("fix the bug"));
    }
}
