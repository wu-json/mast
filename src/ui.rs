use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Cell, Paragraph, Row, Table};
use ratatui::{DefaultTerminal, Frame};

use crate::agents::{self, Agent, Status};
use crate::process::Kind;

const SCAN_EVERY: Duration = Duration::from_millis(1000);
const FRAME: Duration = Duration::from_millis(80);
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

const TEXT: Color = Color::Rgb(228, 228, 231);
const SOFT: Color = Color::Rgb(161, 161, 170);
const DIM: Color = Color::Rgb(113, 113, 122);
const FAINT: Color = Color::Rgb(63, 63, 70);
const ROSE: Color = Color::Rgb(236, 132, 122);
const AMBER: Color = Color::Rgb(226, 190, 128);
const SAGE: Color = Color::Rgb(138, 176, 142);
pub fn run(mut terminal: DefaultTerminal) -> std::io::Result<()> {
    let scans = spawn_scanner();
    let started = Instant::now();
    let mut agents: Option<Vec<Agent>> = None;

    loop {
        while let Ok(latest) = scans.try_recv() {
            agents = Some(latest);
        }
        let tick = (started.elapsed().as_millis() / FRAME.as_millis()) as usize;
        terminal.draw(|f| draw(f, agents.as_deref(), tick))?;

        if event::poll(FRAME)?
            && let Event::Key(key) = event::read()?
        {
            let ctrl_c =
                key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
            if key.kind == KeyEventKind::Press
                && (ctrl_c || matches!(key.code, KeyCode::Char('q') | KeyCode::Esc))
            {
                return Ok(());
            }
        }
    }
}

fn spawn_scanner() -> Receiver<Vec<Agent>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        loop {
            if tx.send(agents::scan()).is_err() {
                return;
            }
            thread::sleep(SCAN_EVERY);
        }
    });
    rx
}

fn draw(f: &mut Frame, agents: Option<&[Agent]>, tick: usize) {
    let area = f.area().inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    });
    let [header, _, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(f, header, agents.unwrap_or_default());

    match agents {
        None => centered(f, body, "scanning the horizon…"),
        Some([]) => centered(f, body, "no agents on the horizon"),
        Some(list) => draw_table(f, body, list, tick),
    }

    f.render_widget(
        Paragraph::new(Line::from(vec!["q".fg(DIM), " quit".fg(FAINT)]))
            .alignment(Alignment::Right),
        footer,
    );
}

fn draw_header(f: &mut Frame, area: Rect, agents: &[Agent]) {
    f.render_widget(
        Paragraph::new(Line::from(vec!["⛵ ".into(), "mast".fg(TEXT).bold()])),
        area,
    );

    let mut counts = Vec::new();
    for status in [Status::Blocked, Status::Working, Status::Idle] {
        let n = agents.iter().filter(|a| a.status == status).count();
        if n == 0 {
            continue;
        }
        if !counts.is_empty() {
            counts.push("  ·  ".fg(FAINT));
        }
        let accent = match status {
            Status::Blocked => ROSE,
            Status::Working => AMBER,
            _ => SAGE,
        };
        counts.push(n.to_string().fg(accent));
        counts.push(format!(" {}", status.label()).fg(DIM));
    }
    f.render_widget(
        Paragraph::new(Line::from(counts)).alignment(Alignment::Right),
        area,
    );
}

struct Tone {
    glyph: Color,
    title: Color,
    status: Color,
    time: Color,
}

fn tone(status: Status) -> Tone {
    match status {
        Status::Blocked => Tone {
            glyph: ROSE,
            title: TEXT,
            status: ROSE,
            time: DIM,
        },
        Status::Working => Tone {
            glyph: AMBER,
            title: TEXT,
            status: AMBER,
            time: DIM,
        },
        Status::Idle => Tone {
            glyph: SAGE,
            title: SOFT,
            status: DIM,
            time: FAINT,
        },
        Status::Unknown => Tone {
            glyph: FAINT,
            title: SOFT,
            status: DIM,
            time: FAINT,
        },
    }
}

fn draw_table(f: &mut Frame, area: Rect, agents: &[Agent], tick: usize) {
    const GAP: u16 = 2;
    const FIXED: [u16; 4] = [1, 6, 9, 4];
    let now = SystemTime::now();
    let title_w = area
        .width
        .saturating_sub(FIXED.iter().sum::<u16>() + GAP * 4) as usize;

    let rows = agents.iter().map(|a| {
        let t = tone(a.status);
        let glyph = match a.status {
            Status::Working => SPINNER[tick % SPINNER.len()],
            Status::Blocked => "◆",
            Status::Idle => "●",
            Status::Unknown => "·",
        };
        let since = now.duration_since(a.since).unwrap_or_default();
        let title = a
            .title
            .clone()
            .or_else(|| {
                Some(
                    a.cwd
                        .as_deref()?
                        .file_name()?
                        .to_string_lossy()
                        .into_owned(),
                )
            })
            .unwrap_or_else(|| "—".to_owned());

        Row::new(vec![
            Cell::from(glyph.fg(t.glyph)),
            Cell::from(a.kind.label().fg(kind_color(a.kind))),
            Cell::from(ellipsize(&title, title_w).fg(t.title)),
            Cell::from(a.status.label().fg(t.status)),
            Cell::from(Line::from(humanize(since).fg(t.time)).alignment(Alignment::Right)),
        ])
    });

    let widths = [
        Constraint::Length(FIXED[0]),
        Constraint::Length(FIXED[1]),
        Constraint::Fill(1),
        Constraint::Length(FIXED[2]),
        Constraint::Length(FIXED[3]),
    ];
    f.render_widget(Table::new(rows, widths).column_spacing(GAP), area);
}

fn ellipsize(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn kind_color(kind: Kind) -> Color {
    match kind {
        Kind::Claude => Color::Rgb(196, 146, 124),
        Kind::Codex => Color::Rgb(134, 162, 199),
        Kind::Pi => Color::Rgb(168, 150, 204),
    }
}

fn centered(f: &mut Frame, area: Rect, text: &str) {
    let [mid] = Layout::vertical([Constraint::Length(1)])
        .flex(ratatui::layout::Flex::Center)
        .areas(area);
    f.render_widget(
        Paragraph::new(text.fg(DIM).italic()).alignment(Alignment::Center),
        mid,
    );
}

pub fn pretty_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && s.starts_with(&home) => format!("~{}", &s[home.len()..]),
        _ => s.into_owned(),
    }
}

pub fn humanize(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn agent(kind: Kind, status: Status, cwd: &str, title: &str, ago: u64) -> Agent {
        Agent {
            kind,
            pid: 1,
            cwd: Some(PathBuf::from(cwd)),
            title: Some(title.to_owned()),
            status,
            since: SystemTime::now() - Duration::from_secs(ago),
        }
    }

    fn render(width: u16, agents: Option<&[Agent]>) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 10)).unwrap();
        terminal.draw(|f| draw(f, agents, 0)).unwrap();
        let buf = terminal.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_agents() {
        let agents = [
            agent(Kind::Claude, Status::Blocked, "/srv/api", "fix-auth", 5),
            agent(
                Kind::Codex,
                Status::Working,
                "/srv/web",
                "Add retry to uploads",
                90,
            ),
            agent(
                Kind::Pi,
                Status::Idle,
                "/srv/docs",
                "write the readme",
                7200,
            ),
        ];
        let out = render(100, Some(&agents));
        println!("{out}");
        assert!(out.contains("mast"));
        assert!(!out.contains("───"));
        assert!(out.contains("1 needs you  ·  1 working  ·  1 idle"));
        assert!(out.contains("claude  fix-auth"));
        assert!(!out.contains("/srv/api"));
        assert!(out.contains("2h"));

        assert!(render(60, Some(&agents)).contains("Add retry to uploads"));
    }

    #[test]
    fn renders_empty_horizon() {
        assert!(render(60, Some(&[])).contains("no agents on the horizon"));
    }
}
