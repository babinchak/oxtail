mod app;
mod config;
mod detail;
mod format;
mod paths;
mod reader;
mod ui;

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, bail, Result};
use clap::Parser;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::app::{App, LogLine};
use crate::config::RuleSet;
use crate::format::Template;

/// A tail for NDJSON logs with a friendly TUI (plain text works too).
#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// File to read (.gz is decompressed on the fly). Reads stdin when omitted.
    file: Option<PathBuf>,

    /// Keep reading as the file grows, like `tail -f` (plain files only).
    #[arg(short, long)]
    follow: bool,

    /// Replay the input at roughly N lines/second (demo mode).
    #[arg(long, value_name = "LINES_PER_SEC")]
    rate: Option<u32>,

    /// Max lines kept in memory; oldest scroll off past this.
    #[arg(long, default_value_t = 50_000)]
    buffer: usize,

    /// One-off format template for every JSON line,
    /// e.g. "{created_at} {type} {repo.name}". Overrides --config.
    #[arg(long, value_name = "TEMPLATE")]
    format: Option<String>,

    /// TOML display-rules file (default: ./oxtail.toml when present).
    /// Edits are live-reloaded while the TUI runs.
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Print N formatted lines to stdout and exit — no TUI. Lets scripts
    /// and AI agents see exactly what a config produces.
    #[arg(long, value_name = "N")]
    render: Option<usize>,

    /// Validate the config (or --format template) and exit.
    #[arg(long)]
    check: bool,

    /// Summarize the stream's JSON structure (paths, types, presence,
    /// shapes) and exit — no TUI. Optionally limit to the first N lines.
    /// Made for humans and AI agents about to write display rules.
    #[arg(long, value_name = "N", num_args = 0..=1, default_missing_value = "0")]
    paths: Option<usize>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let config_path = args.config.clone().or_else(|| {
        let default = PathBuf::from("oxtail.toml");
        default.exists().then_some(default)
    });

    let (rules, config_name) = if let Some(template) = &args.format {
        let t = Template::parse(template).map_err(|e| anyhow!("--format: {e}"))?;
        (RuleSet::from_template(t), Some("--format".to_string()))
    } else if let Some(path) = &config_path {
        let rules = config::load(path).map_err(|e| anyhow!("config error: {e}"))?;
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |f| f.to_string_lossy().into_owned(),
        );
        (rules, Some(name))
    } else {
        (RuleSet::default(), None)
    };

    if args.check {
        match &config_name {
            Some(name) => println!("{name}: ok"),
            None => bail!("nothing to check: no --config, --format, or ./oxtail.toml"),
        }
        return Ok(());
    }

    let source = args
        .file
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "stdin".into());
    let rx = reader::spawn(args.file, args.follow, args.rate);

    // Headless mode: summarize structure and exit.
    if let Some(limit) = args.paths {
        let mut acc = paths::Accumulator::default();
        let mut seen = 0usize;
        while limit == 0 || seen < limit {
            let Ok(raw) = rx.recv() else { break };
            let line = LogLine::parse(raw);
            acc.add(&line.raw, line.json.as_ref());
            seen += 1;
        }
        // Tolerate a closed pipe (e.g. piped into `head`).
        let _ = io::stdout().write_all(acc.report().as_bytes());
        return Ok(());
    }

    // Headless mode: print formatted lines and exit.
    if let Some(n) = args.render {
        let stdout = io::stdout();
        let mut out = stdout.lock();
        for _ in 0..n {
            let Ok(raw) = rx.recv() else { break };
            let line = LogLine::parse(raw);
            let text = ui::line_text(&ui::render_line(&line, &rules, false));
            // A closed pipe (e.g. `| head`) just means we're done.
            if writeln!(out, "{text}").is_err() {
                break;
            }
        }
        return Ok(());
    }

    let mut app = App::new(args.buffer.max(1), source);
    app.rules = rules;
    app.config_name = config_name;

    // Live-reload the config while the TUI runs (not for --format).
    let watch = config_path
        .filter(|_| args.format.is_none())
        .map(|p| (p.clone(), mtime(&p)));

    let terminal = ratatui::init();
    let result = run(terminal, &mut app, rx, watch);
    ratatui::restore();
    result
}

/// Max lines ingested per frame so a firehose can't starve the render loop.
const DRAIN_PER_FRAME: usize = 5_000;

fn run(
    mut terminal: DefaultTerminal,
    app: &mut App,
    rx: Receiver<String>,
    mut watch: Option<(PathBuf, Option<SystemTime>)>,
) -> Result<()> {
    while !app.quit {
        for _ in 0..DRAIN_PER_FRAME {
            match rx.try_recv() {
                Ok(line) => app.push(line),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    app.ended = true;
                    break;
                }
            }
        }

        if let Some((path, seen)) = &mut watch {
            let now = mtime(path);
            if now != *seen {
                *seen = now;
                match config::load(path) {
                    Ok(rules) => {
                        app.rules = rules;
                        app.config_error = None;
                    }
                    Err(e) => app.config_error = Some(e),
                }
            }
        }

        terminal.draw(|f| {
            app.viewport_h = f.area().height.saturating_sub(1) as usize;
            ui::draw(f, app);
        })?;

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
            // Windows delivers both Press and Release events.
            && key.kind == KeyEventKind::Press
        {
            handle_key(app, key.code, key.modifiers);
        }
    }
    Ok(())
}

fn mtime(path: &PathBuf) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn handle_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
        app.quit = true;
        return;
    }

    if let Some(detail) = app.detail.as_mut() {
        let page = detail.page();
        match code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => app.detail = None,
            KeyCode::Up | KeyCode::Char('k') => detail.scroll_up(1),
            KeyCode::Down | KeyCode::Char('j') => detail.scroll_down(1),
            KeyCode::PageUp => detail.scroll_up(page),
            KeyCode::PageDown => detail.scroll_down(page),
            KeyCode::Home | KeyCode::Char('g') => detail.jump_to_top(),
            KeyCode::End | KeyCode::Char('G') => detail.jump_to_bottom(),
            _ => {}
        }
        return;
    }

    let page = app.viewport_h.max(1);
    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.quit = true,
        KeyCode::Enter => app.open_detail(),
        KeyCode::Char('r') => app.raw_mode = !app.raw_mode,
        KeyCode::Up | KeyCode::Char('k') => app.scroll_up(1),
        KeyCode::Down | KeyCode::Char('j') => app.scroll_down(1),
        KeyCode::PageUp => app.scroll_up(page),
        KeyCode::PageDown => app.scroll_down(page),
        KeyCode::Home | KeyCode::Char('g') => app.scroll_to_top(),
        KeyCode::End | KeyCode::Char('G') | KeyCode::Char('f') => app.resume_follow(),
        _ => {}
    }
}
