mod agents;
mod process;
mod ui;

use std::time::SystemTime;

const HELP: &str = "\
mast — watch the coding agents running on this machine

usage: mast [--once]

  --once     print a tab-separated snapshot and exit:
             agent, status, time in status, pid, cwd, title
  -h, --help show this help
  -V         print version

keys: q / esc quit";

fn main() -> std::io::Result<()> {
    match std::env::args().nth(1).as_deref() {
        None => {
            let terminal = ratatui::init();
            let result = ui::run(terminal);
            ratatui::restore();
            result
        }
        Some("--once") => {
            once();
            Ok(())
        }
        Some("-V" | "--version") => {
            println!("mast {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("-h" | "--help") => {
            println!("{HELP}");
            Ok(())
        }
        Some(other) => {
            eprintln!("unknown argument: {other}\n\n{HELP}");
            std::process::exit(2);
        }
    }
}

fn once() {
    let now = SystemTime::now();
    for a in agents::scan() {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            a.kind.label(),
            a.status.label(),
            ui::humanize(now.duration_since(a.since).unwrap_or_default()),
            a.pid,
            a.cwd.as_deref().map(ui::pretty_path).unwrap_or_default(),
            a.title.unwrap_or_default(),
        );
    }
}
