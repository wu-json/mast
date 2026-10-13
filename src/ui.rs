use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::agents::{self, Agent, Status};
use crate::process::Kind;
use crate::term::{Key, Tty};

const SCAN_EVERY: Duration = Duration::from_millis(1000);
const FRAME: Duration = Duration::from_millis(80);
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

const TEXT: Color = Color(228, 228, 231);
const SOFT: Color = Color(161, 161, 170);
const DIM: Color = Color(113, 113, 122);
const FAINT: Color = Color(63, 63, 70);
const ROSE: Color = Color(236, 132, 122);
const AMBER: Color = Color(226, 190, 128);
const SAGE: Color = Color(138, 176, 142);

const PUSH_TITLE: &str = "\x1b[22;0t";
const POP_TITLE: &str = "\x1b[23;0t";

pub fn run() -> io::Result<()> {
    let mut tty = Tty::open()?;
    let mut out = io::stdout().lock();
    write!(out, "{PUSH_TITLE}")?;
    let result = watch(&mut tty, &mut out);
    write!(out, "{POP_TITLE}")?;
    out.flush()?;
    result
}

fn watch(tty: &mut Tty, out: &mut impl Write) -> io::Result<()> {
    let scans = spawn_scanner();
    let started = Instant::now();
    let mut agents: Option<Vec<Agent>> = None;
    let mut shown_title = String::new();
    let mut shown: Option<Canvas> = None;

    loop {
        while let Ok(latest) = scans.try_recv() {
            agents = Some(latest);
        }
        let title = title(agents.as_deref().unwrap_or_default());
        if title != shown_title {
            write!(out, "\x1b]0;{title}\x07")?;
            shown_title = title;
        }
        let tick = (started.elapsed().as_millis() / FRAME.as_millis()) as usize;
        let (w, h) = tty.size();
        let mut canvas = Canvas::new(w, h);
        draw(&mut canvas, agents.as_deref(), tick);
        out.write_all(canvas.diff(shown.as_ref()).as_bytes())?;
        out.flush()?;
        shown = Some(canvas);

        if let Some(Key::Quit) = tty.key(FRAME)? {
            return Ok(());
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

#[derive(Clone, Copy, PartialEq, Eq)]
struct Color(u8, u8, u8);

#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct Style {
    fg: Option<Color>,
    bold: bool,
    italic: bool,
}

impl Style {
    fn bold(self) -> Style {
        Style { bold: true, ..self }
    }

    fn italic(self) -> Style {
        Style {
            italic: true,
            ..self
        }
    }
}

fn fg(color: Color) -> Style {
    Style {
        fg: Some(color),
        ..Style::default()
    }
}

type Span<'a> = (&'a str, Style);

#[derive(Clone, Copy)]
enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy)]
struct Rect {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Cell(char, Style);

struct Canvas {
    width: u16,
    height: u16,
    cells: Vec<Cell>,
}

impl Canvas {
    fn new(width: u16, height: u16) -> Canvas {
        let blank = Cell(' ', Style::default());
        Canvas {
            width,
            height,
            cells: vec![blank; width as usize * height as usize],
        }
    }

    fn area(&self) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        }
    }

    fn line(&mut self, area: Rect, y: u16, spans: &[Span], align: Align) {
        if y >= area.height {
            return;
        }
        let used: usize = spans.iter().map(|(s, _)| s.width()).sum();
        let slack = (area.width as usize).saturating_sub(used) as u16;
        let mut x = area.x
            + match align {
                Align::Left => 0,
                Align::Center => slack / 2,
                Align::Right => slack,
            };
        let end = area.x + area.width;
        let row = (area.y + y) as usize * self.width as usize;
        for &(text, style) in spans {
            for ch in text.chars() {
                let w = ch.width().unwrap_or(0) as u16;
                if w == 0 {
                    continue;
                }
                if x + w > end {
                    return;
                }
                self.cells[row + x as usize] = Cell(ch, style);
                if w == 2 {
                    self.cells[row + x as usize + 1] = Cell('\0', style);
                }
                x += w;
            }
        }
    }

    fn row(&self, y: u16) -> &[Cell] {
        let w = self.width as usize;
        &self.cells[y as usize * w..][..w]
    }

    fn diff(&self, prev: Option<&Canvas>) -> String {
        let prev = prev.filter(|p| (p.width, p.height) == (self.width, self.height));
        let mut out = String::new();
        if prev.is_none() {
            out.push_str("\x1b[2J");
        }
        for y in 0..self.height {
            let row = self.row(y);
            if prev.is_some_and(|p| p.row(y) == row) {
                continue;
            }
            let _ = write!(out, "\x1b[{};1H", y + 1);
            let mut style = None;
            for &Cell(ch, s) in row {
                if ch == '\0' {
                    continue;
                }
                if style != Some(s) {
                    out.push_str("\x1b[0");
                    if let Some(Color(r, g, b)) = s.fg {
                        let _ = write!(out, ";38;2;{r};{g};{b}");
                    }
                    if s.bold {
                        out.push_str(";1");
                    }
                    if s.italic {
                        out.push_str(";3");
                    }
                    out.push('m');
                    style = Some(s);
                }
                out.push(ch);
            }
            out.push_str("\x1b[0m");
        }
        if !out.is_empty() {
            out.insert_str(0, "\x1b[?2026h");
            out.push_str("\x1b[?2026l");
        }
        out
    }
}

fn draw(c: &mut Canvas, agents: Option<&[Agent]>, tick: usize) {
    let full = c.area();
    let area = Rect {
        x: 2.min(full.width),
        y: 1.min(full.height),
        width: full.width.saturating_sub(4),
        height: full.height.saturating_sub(2),
    };
    if area.height == 0 {
        return;
    }
    let body = Rect {
        y: area.y + 2,
        height: area.height.saturating_sub(3),
        ..area
    };

    draw_header(c, area, agents.unwrap_or_default());

    match agents {
        None => centered(c, body, "scanning the horizon…"),
        Some([]) => centered(c, body, "no agents on the horizon"),
        Some(list) => draw_table(c, body, list, tick),
    }

    if area.height >= 4 {
        c.line(
            area,
            area.height - 1,
            &[("q", fg(DIM)), (" quit", fg(FAINT))],
            Align::Right,
        );
    }
}

fn counts(agents: &[Agent]) -> impl Iterator<Item = (Status, usize)> {
    [Status::Blocked, Status::Working, Status::Idle]
        .into_iter()
        .map(|status| (status, agents.iter().filter(|a| a.status == status).count()))
        .filter(|&(_, n)| n > 0)
}

fn title(agents: &[Agent]) -> String {
    let mut out = "⛵ mast".to_owned();
    for (status, n) in counts(agents) {
        out.push_str(&format!(" · {n} {}", status.label()));
    }
    out
}

fn draw_header(c: &mut Canvas, area: Rect, agents: &[Agent]) {
    c.line(
        area,
        0,
        &[("⛵ ", Style::default()), ("mast", fg(TEXT).bold())],
        Align::Left,
    );

    let counts: Vec<_> = counts(agents)
        .map(|(status, n)| {
            let accent = match status {
                Status::Blocked => ROSE,
                Status::Working => AMBER,
                _ => SAGE,
            };
            (n.to_string(), format!(" {}", status.label()), accent)
        })
        .collect();
    let mut spans = Vec::new();
    for (n, label, accent) in &counts {
        if !spans.is_empty() {
            spans.push(("  ·  ", fg(FAINT)));
        }
        spans.push((n.as_str(), fg(*accent)));
        spans.push((label.as_str(), fg(DIM)));
    }
    c.line(area, 0, &spans, Align::Right);
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

fn draw_table(c: &mut Canvas, area: Rect, agents: &[Agent], tick: usize) {
    const GAP: u16 = 2;
    const FIXED: [u16; 4] = [1, 6, 9, 4];
    let now = SystemTime::now();
    let title_w = area
        .width
        .saturating_sub(FIXED.iter().sum::<u16>() + GAP * 4);
    let widths = [FIXED[0], FIXED[1], title_w, FIXED[2], FIXED[3]];

    for (y, a) in (0..area.height).zip(agents) {
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

        let title = ellipsize(&title, title_w as usize);
        let time = humanize(since);
        let cells = [
            (glyph, t.glyph, Align::Left),
            (a.kind.label(), kind_color(a.kind), Align::Left),
            (title.as_str(), t.title, Align::Left),
            (a.status.label(), t.status, Align::Left),
            (time.as_str(), t.time, Align::Right),
        ];
        let mut x = area.x;
        for ((text, color, align), width) in cells.into_iter().zip(widths) {
            let width = width.min((area.x + area.width).saturating_sub(x));
            let cell = Rect { x, width, ..area };
            c.line(cell, y, &[(text, fg(color))], align);
            x += width + GAP;
        }
    }
}

fn ellipsize(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

fn kind_color(kind: Kind) -> Color {
    match kind {
        Kind::Claude => Color(196, 146, 124),
        Kind::Codex => Color(134, 162, 199),
        Kind::Pi => Color(168, 150, 204),
    }
}

fn centered(c: &mut Canvas, area: Rect, text: &str) {
    if area.height > 0 {
        c.line(
            area,
            (area.height - 1) / 2,
            &[(text, fg(DIM).italic())],
            Align::Center,
        );
    }
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
        let mut canvas = Canvas::new(width, 10);
        draw(&mut canvas, agents, 0);
        (0..canvas.height)
            .map(|y| {
                canvas
                    .row(y)
                    .iter()
                    .map(|cell| cell.0)
                    .filter(|&ch| ch != '\0')
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
    fn titles_with_counts() {
        assert_eq!(title(&[]), "⛵ mast");
        let agents = [
            agent(Kind::Claude, Status::Idle, "/srv/api", "a", 1),
            agent(Kind::Codex, Status::Blocked, "/srv/web", "b", 1),
            agent(Kind::Pi, Status::Idle, "/srv/docs", "c", 1),
        ];
        assert_eq!(title(&agents), "⛵ mast · 1 needs you · 2 idle");
    }

    #[test]
    fn renders_empty_horizon() {
        assert!(render(60, Some(&[])).contains("no agents on the horizon"));
    }
}
