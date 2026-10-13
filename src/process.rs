//! Process table discovery for coding agents.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Claude,
    Codex,
    Pi,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Claude => "claude",
            Kind::Codex => "codex",
            Kind::Pi => "pi",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub started: SystemTime,
    pub args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AgentProc {
    pub kind: Kind,
    pub proc: Proc,
    pub cwd: Option<PathBuf>,
}

/// Every running claude / codex / pi process, with wrapper processes collapsed.
pub fn scan() -> Vec<AgentProc> {
    let procs = list_processes();
    let by_pid: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();

    let found: Vec<(Kind, &Proc)> = procs
        .iter()
        .filter_map(|p| identify(&p.args).map(|k| (k, p)))
        .collect();

    // A launcher (npm shim, shell wrapper) and the real agent both match; keep
    // the innermost process so open files and state belong to the real agent.
    let mut wrappers = Vec::new();
    for (kind, p) in &found {
        let mut cur = p.ppid;
        for _ in 0..16 {
            let Some(parent) = by_pid.get(&cur) else {
                break;
            };
            if identify(&parent.args) == Some(*kind) {
                wrappers.push(parent.pid);
            }
            cur = parent.ppid;
        }
    }

    let agents: Vec<(Kind, &Proc)> = found
        .into_iter()
        .filter(|(_, p)| !wrappers.contains(&p.pid))
        .collect();

    let pids: Vec<u32> = agents.iter().map(|(_, p)| p.pid).collect();
    let cwds = cwds(&pids);

    agents
        .into_iter()
        .map(|(kind, p)| AgentProc {
            kind,
            proc: p.clone(),
            cwd: cwds.get(&p.pid).cloned(),
        })
        .collect()
}

/// Mirrors herdr's process-name lookup, narrowed to the agents we track.
fn identify(args: &[String]) -> Option<Kind> {
    let argv0 = basename(args.first()?);
    let name = match argv0 {
        // Script installs run as `node /path/to/pi` or `bun .../codex.js`.
        "node" | "bun" | "deno" => args
            .iter()
            .skip(1)
            .find(|a| !a.starts_with('-'))
            .map(|a| strip_script_ext(basename(a)))?,
        other => other,
    };
    let kind = match name {
        "claude" | "claude-code" => Kind::Claude,
        "codex" => Kind::Codex,
        "pi" => Kind::Pi,
        _ => return None,
    };
    // Codex ships long-lived background servers under the same binary name.
    if kind == Kind::Codex
        && args
            .iter()
            .any(|a| matches!(a.as_str(), "app-server" | "mcp-server" | "mcp" | "login"))
    {
        return None;
    }
    Some(kind)
}

fn basename(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

fn strip_script_ext(s: &str) -> &str {
    [".js", ".mjs", ".cjs", ".ts"]
        .iter()
        .find_map(|ext| s.strip_suffix(ext))
        .unwrap_or(s)
}

fn list_processes() -> Vec<Proc> {
    let Ok(out) = Command::new("ps")
        .args(["-axww", "-o", "pid=,ppid=,etime=,args="])
        .output()
    else {
        return Vec::new();
    };
    let now = SystemTime::now();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let ppid = it.next()?.parse().ok()?;
            let elapsed = parse_etime(it.next()?)?;
            let args: Vec<String> = it.map(str::to_owned).collect();
            if args.is_empty() {
                return None;
            }
            Some(Proc {
                pid,
                ppid,
                started: now.checked_sub(elapsed).unwrap_or(now),
                args,
            })
        })
        .collect()
}

/// `[[dd-]hh:]mm:ss`
fn parse_etime(s: &str) -> Option<Duration> {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, s),
    };
    let mut secs = 0u64;
    for part in rest.split(':') {
        secs = secs * 60 + part.parse::<u64>().ok()?;
    }
    Some(Duration::from_secs(days * 86_400 + secs))
}

#[cfg(target_os = "linux")]
fn cwds(pids: &[u32]) -> HashMap<u32, PathBuf> {
    pids.iter()
        .filter_map(|&pid| {
            std::fs::read_link(format!("/proc/{pid}/cwd"))
                .ok()
                .map(|p| (pid, p))
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn cwds(pids: &[u32]) -> HashMap<u32, PathBuf> {
    lsof(pids, &["-d", "cwd"])
        .into_iter()
        .filter_map(|(pid, mut files)| files.pop().map(|f| (pid, f)))
        .collect()
}

/// Files the given processes currently hold open.
pub fn open_files(pids: &[u32]) -> HashMap<u32, Vec<PathBuf>> {
    #[cfg(target_os = "linux")]
    {
        pids.iter()
            .map(|&pid| {
                let files = std::fs::read_dir(format!("/proc/{pid}/fd"))
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter_map(|e| std::fs::read_link(e.path()).ok())
                    .collect();
                (pid, files)
            })
            .collect()
    }
    #[cfg(not(target_os = "linux"))]
    {
        lsof(pids, &[])
    }
}

#[cfg(not(target_os = "linux"))]
fn lsof(pids: &[u32], extra: &[&str]) -> HashMap<u32, Vec<PathBuf>> {
    let mut map: HashMap<u32, Vec<PathBuf>> = HashMap::new();
    if pids.is_empty() {
        return map;
    }
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let mut cmd = Command::new("lsof");
    cmd.args(["-n", "-P", "-w", "-a", "-p", &list])
        .args(extra)
        .arg("-Fpn");
    let Ok(out) = cmd.output() else { return map };
    let mut current = None;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(pid) = line.strip_prefix('p') {
            current = pid.parse().ok();
        } else if let (Some(path), Some(pid)) = (line.strip_prefix('n'), current)
            && path.starts_with('/')
        {
            map.entry(pid).or_default().push(PathBuf::from(path));
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn identifies_agents() {
        assert_eq!(identify(&args("claude --resume")), Some(Kind::Claude));
        assert_eq!(
            identify(&args("/opt/homebrew/bin/codex")),
            Some(Kind::Codex)
        );
        assert_eq!(
            identify(&args(
                "node /usr/lib/node_modules/@openai/codex/bin/codex.js"
            )),
            Some(Kind::Codex)
        );
        assert_eq!(identify(&args("/usr/local/bin/pi -c")), Some(Kind::Pi));
        assert_eq!(identify(&args("codex app-server --listen unix://")), None);
        assert_eq!(
            identify(&args("/Applications/Claude.app/Contents/MacOS/Claude")),
            None
        );
        assert_eq!(identify(&args("pip install foo")), None);
    }

    #[test]
    fn parses_etime() {
        assert_eq!(parse_etime("05:03"), Some(Duration::from_secs(303)));
        assert_eq!(parse_etime("01:00:00"), Some(Duration::from_secs(3600)));
        assert_eq!(
            parse_etime("2-00:00:01"),
            Some(Duration::from_secs(172_801))
        );
    }
}
