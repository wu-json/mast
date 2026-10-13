use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::process::{self, AgentProc, Kind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
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
    pub since: SystemTime,
}

pub fn scan() -> Vec<Agent> {
    let procs = process::scan();
    let home = home();

    let codex_pids: Vec<u32> = procs
        .iter()
        .filter(|p| matches!(p.kind, Kind::Codex | Kind::Pi))
        .map(|p| p.proc.pid)
        .collect();
    let open = process::open_files(&codex_pids);
    let mut claimed: HashSet<PathBuf> = open
        .values()
        .flatten()
        .filter(|f| is_jsonl(f))
        .cloned()
        .collect();

    let opencode_sessions = procs
        .iter()
        .filter(|p| p.kind == Kind::OpenCode)
        .map(|p| p.proc.started)
        .min()
        .and_then(|started| opencode_sessions(&home, started));
    let opencode_assigned = opencode_sessions
        .as_deref()
        .map(|sessions| opencode_assign(&procs, sessions));

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
                Kind::OpenCode => opencode(&mut agent, opencode_assigned.as_ref()),
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

static TITLES: LazyLock<Mutex<HashMap<PathBuf, TitleScan>>> = LazyLock::new(Default::default);

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
                break;
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

struct OpenCodeSession {
    id: String,
    directory: PathBuf,
    title: String,
    created: SystemTime,
    updated: SystemTime,
    status: Status,
    since: SystemTime,
}

fn opencode_sessions(home: &Path, started: SystemTime) -> Option<Vec<OpenCodeSession>> {
    let data = env_dir("XDG_DATA_HOME", || home.join(".local").join("share")).join("opencode");
    let db = match std::env::var_os("OPENCODE_DB") {
        Some(db) if db == ":memory:" => return None,
        Some(db) => data.join(db),
        None => data.join("opencode.db"),
    };
    if !db.exists() {
        return None;
    }
    let floor = millis(session_floor(started));
    let query = format!(
        "SELECT s.id, s.directory, s.title, s.time_created AS created, s.time_updated AS updated, \
           (SELECT json_object('role', json_extract(m.data, '$.role'), \
                               'finish', json_extract(m.data, '$.finish'), \
                               'time', json_object('created', m.time_created, \
                                                   'completed', json_extract(m.data, '$.time.completed'))) \
            FROM message m WHERE m.session_id = s.id \
            ORDER BY m.time_created DESC, m.id DESC LIMIT 1) AS last, \
           (SELECT max(m.time_created) FROM message m \
            WHERE m.session_id = s.id AND json_extract(m.data, '$.role') = 'user') AS turn \
         FROM session s WHERE s.parent_id IS NULL AND s.time_updated >= {floor} \
         ORDER BY s.time_updated DESC"
    );
    let out = Command::new("sqlite3")
        .args(["-readonly", "-json", "-cmd", ".timeout 250"])
        .arg(&db)
        .arg(query)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    if out.stdout.iter().all(u8::is_ascii_whitespace) {
        return Some(Vec::new());
    }
    let rows: Vec<Value> = serde_json::from_slice(&out.stdout).ok()?;
    Some(rows.iter().filter_map(opencode_session).collect())
}

fn opencode_session(row: &Value) -> Option<OpenCodeSession> {
    let at = |v: &Value| v.as_u64().map(|ms| UNIX_EPOCH + Duration::from_millis(ms));
    let updated = at(&row["updated"])?;
    let last: Value = row["last"]
        .as_str()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let status = opencode_message_status(&last);
    let since = match status {
        Status::Working => at(&row["turn"]).or_else(|| at(&last["time"]["created"])),
        _ => at(&last["time"]["completed"]),
    };
    Some(OpenCodeSession {
        id: row["id"].as_str()?.to_owned(),
        directory: PathBuf::from(row["directory"].as_str()?),
        title: row["title"].as_str().unwrap_or_default().to_owned(),
        created: at(&row["created"])?,
        updated,
        status,
        since: since.unwrap_or(updated),
    })
}

static OPENCODE_ASSIGNED: LazyLock<Mutex<HashMap<u32, String>>> = LazyLock::new(Default::default);

fn opencode_assign<'s>(
    procs: &[AgentProc],
    sessions: &'s [OpenCodeSession],
) -> HashMap<u32, &'s OpenCodeSession> {
    let procs: Vec<&AgentProc> = procs.iter().filter(|p| p.kind == Kind::OpenCode).collect();
    let Ok(mut previous) = OPENCODE_ASSIGNED.lock() else {
        return HashMap::new();
    };
    let assigned = opencode_match(&procs, sessions, &previous);
    *previous = assigned
        .iter()
        .map(|(&pid, s)| (pid, s.id.clone()))
        .collect();
    assigned
}

fn opencode_match<'s>(
    procs: &[&AgentProc],
    sessions: &'s [OpenCodeSession],
    previous: &HashMap<u32, String>,
) -> HashMap<u32, &'s OpenCodeSession> {
    let fits = |p: &AgentProc, s: &OpenCodeSession| {
        Some(&s.directory) == p.cwd.as_ref() && s.updated >= session_floor(p.proc.started)
    };
    let mut taken: HashSet<&str> = HashSet::new();
    let mut assigned: HashMap<u32, &OpenCodeSession> = HashMap::new();

    for p in procs {
        let explicit = opencode_session_arg(&p.proc.args);
        let wanted = explicit.or_else(|| previous.get(&p.proc.pid).map(String::as_str));
        let Some(s) = wanted.and_then(|id| sessions.iter().find(|s| s.id == id)) else {
            continue;
        };
        if (explicit.is_some() || fits(p, s)) && taken.insert(&s.id) {
            assigned.insert(p.proc.pid, s);
        }
    }

    let mut rest: Vec<&AgentProc> = procs
        .iter()
        .copied()
        .filter(|p| opencode_session_arg(&p.proc.args).is_none())
        .collect();
    rest.sort_by_key(|p| {
        (
            assigned.contains_key(&p.proc.pid),
            std::cmp::Reverse(p.proc.started),
        )
    });
    for p in rest {
        let current = assigned.get(&p.proc.pid).map(|s| s.updated);
        let floor = session_floor(p.proc.started);
        let Some(s) = sessions
            .iter()
            .filter(|s| {
                fits(p, s)
                    && !taken.contains(s.id.as_str())
                    && current.is_none_or(|t| s.updated > t)
            })
            .max_by_key(|s| (s.created >= floor, s.updated))
        else {
            continue;
        };
        if let Some(old) = assigned.insert(p.proc.pid, s) {
            taken.remove(old.id.as_str());
        }
        taken.insert(&s.id);
    }
    assigned
}

fn opencode_session_arg(args: &[String]) -> Option<&str> {
    if args.iter().any(|a| a == "--fork") {
        return None;
    }
    args.iter().enumerate().find_map(|(i, a)| match a.as_str() {
        "-s" | "--session" => args.get(i + 1).map(String::as_str),
        _ => a.strip_prefix("--session="),
    })
}

fn opencode(agent: &mut Agent, assigned: Option<&HashMap<u32, &OpenCodeSession>>) {
    let Some(assigned) = assigned else { return };
    agent.status = Status::Idle;
    let Some(session) = assigned.get(&agent.pid) else {
        return;
    };
    agent.title =
        Some(session.title.clone()).filter(|t| !t.is_empty() && !t.starts_with("New session - "));
    if session.status != Status::Unknown {
        agent.status = session.status;
        agent.since = session.since;
    }
}

fn opencode_message_status(v: &Value) -> Status {
    match (
        v["role"].as_str(),
        v["time"]["completed"].as_u64(),
        v["finish"].as_str(),
    ) {
        (Some("assistant"), Some(_), Some("tool-calls" | "unknown")) => Status::Working,
        (Some("assistant"), Some(_), _) => Status::Idle,
        (Some("user" | "assistant"), _, _) => Status::Working,
        _ => Status::Unknown,
    }
}

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

fn session_floor(started: SystemTime) -> SystemTime {
    started
        .checked_sub(Duration::from_secs(5))
        .unwrap_or(UNIX_EPOCH)
}

fn millis(t: SystemTime) -> u128 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

fn newest_matching(
    files: Vec<PathBuf>,
    p: &AgentProc,
    claimed: &HashSet<PathBuf>,
    accept: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let floor = session_floor(p.proc.started);
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
        lines.next();
    }
    lines.filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    let s = s.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let (hms, frac) = time.split_once('.').unwrap_or((time, ""));
    let mut t = hms.split(':').map(|x| x.parse::<i64>());
    let (hh, mm, ss) = (t.next()?.ok()?, t.next()?.ok()?, t.next()?.ok()?);
    let nanos: u32 = format!("{frac:0<9}").get(..9)?.parse().ok()?;

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
    fn reads_opencode_turns() {
        let status = |v: Value| opencode_message_status(&v);
        assert_eq!(
            status(serde_json::json!({"role": "user", "time": {"created": 1}})),
            Status::Working
        );
        assert_eq!(
            status(serde_json::json!({"role": "assistant", "time": {"created": 1}})),
            Status::Working
        );
        assert_eq!(
            status(serde_json::json!({
                "role": "assistant", "time": {"created": 1, "completed": 2}, "finish": "tool-calls"
            })),
            Status::Working
        );
        assert_eq!(
            status(serde_json::json!({
                "role": "assistant", "time": {"created": 1, "completed": 2}, "finish": "stop"
            })),
            Status::Idle
        );
    }

    #[test]
    fn reads_opencode_session_rows() {
        let row = |last: &str| {
            serde_json::json!({
                "id": "ses_1", "directory": "/srv/api", "title": "Fix auth",
                "created": 500, "updated": 3000, "turn": 1000, "last": last,
            })
        };
        let done = opencode_session(&row(
            r#"{"role":"assistant","finish":"stop","time":{"created":2000,"completed":2500}}"#,
        ))
        .unwrap();
        assert_eq!(done.status, Status::Idle);
        assert_eq!(done.since, UNIX_EPOCH + Duration::from_millis(2500));

        let busy = opencode_session(&row(
            r#"{"role":"assistant","finish":null,"time":{"created":2000,"completed":null}}"#,
        ))
        .unwrap();
        assert_eq!(busy.status, Status::Working);
        assert_eq!(busy.since, UNIX_EPOCH + Duration::from_millis(1000));
    }

    #[test]
    fn keeps_opencode_sessions_with_their_process() {
        let at = |s: u64| UNIX_EPOCH + Duration::from_secs(s);
        let proc = |pid: u32, started: u64, args: &str| AgentProc {
            kind: Kind::OpenCode,
            proc: process::Proc {
                pid,
                ppid: 1,
                started: at(started),
                args: args.split_whitespace().map(str::to_owned).collect(),
            },
            cwd: Some(PathBuf::from("/srv/api")),
        };
        let session = |id: &str, created: u64, updated: u64| OpenCodeSession {
            id: id.to_owned(),
            directory: PathBuf::from("/srv/api"),
            title: id.to_owned(),
            created: at(created),
            updated: at(updated),
            status: Status::Idle,
            since: at(updated),
        };
        let (a, b) = (proc(1, 100, "opencode"), proc(2, 100, "opencode"));
        let procs = [&a, &b];
        let pick = |procs: &[&AgentProc],
                    sessions: &[OpenCodeSession],
                    previous: &HashMap<u32, String>| {
            opencode_match(procs, sessions, previous)
                .into_iter()
                .map(|(pid, s)| (pid, s.id.clone()))
                .collect::<HashMap<_, _>>()
        };

        let first = pick(&procs, &[session("s1", 150, 200)], &HashMap::new());
        assert_eq!(first.len(), 1);
        let owner = *first.keys().next().unwrap();
        let other = 3 - owner;

        let sessions = [session("s2", 250, 300), session("s1", 150, 200)];
        let second = pick(&procs, &sessions, &first);
        assert_eq!(second[&owner], "s1");
        assert_eq!(second[&other], "s2");

        let sessions = [session("s1", 150, 400), session("s2", 250, 300)];
        assert_eq!(pick(&procs, &sessions, &second), second);

        let (early, late) = (proc(1, 100, "opencode"), proc(2, 200, "opencode"));
        let sessions = [session("s2", 250, 400), session("s1", 150, 300)];
        let fresh = pick(&[&early, &late], &sessions, &HashMap::new());
        assert_eq!(fresh[&1], "s1");
        assert_eq!(fresh[&2], "s2");

        let pinned = proc(3, 100, "opencode -s s2");
        assert_eq!(pick(&[&pinned], &sessions, &HashMap::new())[&3], "s2");
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
