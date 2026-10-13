use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Claude,
    Codex,
    OpenCode,
    Pi,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Claude => "claude",
            Kind::Codex => "codex",
            Kind::OpenCode => "opencode",
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

pub fn scan() -> Vec<AgentProc> {
    let procs = list_processes();
    let by_pid: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();

    let found: Vec<(Kind, &Proc)> = procs
        .iter()
        .filter_map(|p| identify(&p.args).map(|k| (k, p)))
        .collect();

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

fn identify(args: &[String]) -> Option<Kind> {
    let argv0 = basename(args.first()?);
    let (at, name) = match argv0 {
        "node" | "bun" | "deno" => args
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, a)| !a.starts_with('-'))
            .map(|(i, a)| (i, strip_script_ext(basename(a))))?,
        other => (0, other),
    };
    let kind = match name {
        "claude" | "claude-code" => Kind::Claude,
        "codex" => Kind::Codex,
        "opencode" => Kind::OpenCode,
        "pi" => Kind::Pi,
        _ => return None,
    };
    if kind == Kind::Codex
        && args
            .iter()
            .any(|a| matches!(a.as_str(), "app-server" | "mcp-server" | "mcp" | "login"))
    {
        return None;
    }
    if kind == Kind::OpenCode
        && args
            .get(at + 1)
            .is_some_and(|a| matches!(a.as_str(), "serve" | "web"))
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
        assert_eq!(identify(&args("opencode")), Some(Kind::OpenCode));
        assert_eq!(
            identify(&args(
                "node /usr/lib/node_modules/opencode-ai/bin/opencode -c"
            )),
            Some(Kind::OpenCode)
        );
        assert_eq!(
            identify(&args("opencode run fix the serve command")),
            Some(Kind::OpenCode)
        );
        assert_eq!(identify(&args("opencode serve --port 4096")), None);
        assert_eq!(identify(&args("bun /opt/opencode/bin/opencode web")), None);
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
