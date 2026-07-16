mod app;
mod config;
mod detail;
mod format;
mod paths;
mod reader;
mod ui;

use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, bail, Result};
use clap::{Args, Parser, Subcommand};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::app::{App, LogLine};
use crate::config::RuleSet;
use crate::format::Template;

/// The AI agent skill ships inside the binary so installed copies are
/// self-describing and the docs can never drift from the features.
const SKILL_MD: &str = include_str!("../skills/oxtail-format/SKILL.md");

/// A tail for NDJSON logs with a friendly TUI (plain text works too).
#[derive(Parser)]
#[command(
    version,
    about,
    after_help = "AI agents: run `oxtail skill` for the bundled guide to writing display-rule configs."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

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

    #[command(flatten)]
    fmt: FormatArgs,
}

#[derive(Args)]
struct FormatArgs {
    /// One-off format template for every JSON line,
    /// e.g. "{created_at} {type} {repo.name}". Overrides --config.
    #[arg(long, value_name = "TEMPLATE")]
    format: Option<String>,

    /// TOML display-rules file (default: ./oxtail.toml when present).
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Print formatted lines to stdout — no TUI. Lets scripts and AI agents
    /// see exactly what a config produces.
    Render {
        /// File to read (.gz ok). Reads stdin when omitted.
        file: Option<PathBuf>,
        /// Stop after N input lines (default: read to the end).
        #[arg(short = 'n', long, value_name = "N")]
        lines: Option<usize>,
        #[command(flatten)]
        fmt: FormatArgs,
    },
    /// Validate a rules config (or --format template) and exit.
    Check {
        #[command(flatten)]
        fmt: FormatArgs,
    },
    /// Summarize the stream's JSON structure (paths, types, presence,
    /// shapes) — for humans and AI agents about to write display rules.
    Paths {
        /// File to read (.gz ok). Reads stdin when omitted.
        file: Option<PathBuf>,
        /// Stop after N input lines (default: read to the end).
        #[arg(short = 'n', long, value_name = "N")]
        lines: Option<usize>,
    },
    /// Print the bundled AI agent skill for writing display rules.
    Skill {
        #[command(subcommand)]
        action: Option<SkillAction>,
    },
}

#[derive(Subcommand)]
enum SkillAction {
    /// Install the skill for Claude Code (~/.claude/skills/oxtail-format/).
    Install,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Skill { action: None }) => {
            let _ = io::stdout().write_all(SKILL_MD.as_bytes());
            Ok(())
        }
        Some(Command::Skill {
            action: Some(SkillAction::Install),
        }) => install_skill(),
        Some(Command::Check { fmt }) => {
            let (_, name, _) = load_rules(&fmt)?;
            match name {
                Some(n) => {
                    println!("{n}: ok");
                    Ok(())
                }
                None => bail!("nothing to check: no --config, --format, or ./oxtail.toml"),
            }
        }
        Some(Command::Paths { file, lines }) => run_paths(file, lines),
        Some(Command::Render { file, lines, fmt }) => run_render(file, lines, &fmt),
        None => run_tui(cli),
    }
}

/// Resolve display rules from --format, --config, or ./oxtail.toml.
/// Returns (rules, display name, config path to watch for live reload).
fn load_rules(fmt: &FormatArgs) -> Result<(RuleSet, Option<String>, Option<PathBuf>)> {
    if let Some(template) = &fmt.format {
        let t = Template::parse(template).map_err(|e| anyhow!("--format: {e}"))?;
        return Ok((RuleSet::from_template(t), Some("--format".into()), None));
    }
    let path = fmt.config.clone().or_else(|| {
        let default = PathBuf::from("oxtail.toml");
        default.exists().then_some(default)
    });
    match path {
        Some(p) => {
            let rules = config::load(&p).map_err(|e| anyhow!("config error: {e}"))?;
            let name = p.file_name().map_or_else(
                || p.display().to_string(),
                |f| f.to_string_lossy().into_owned(),
            );
            Ok((rules, Some(name), Some(p)))
        }
        None => Ok((RuleSet::default(), None, None)),
    }
}

/// Reading stdin only makes sense when something is piped in. Without this
/// guard, a bare invocation from a terminal blocks on keyboard "input" —
/// and in the TUI the stdin reader even eats the keystrokes meant for it.
fn require_input(file: &Option<PathBuf>) -> Result<()> {
    if file.is_none() && io::stdin().is_terminal() {
        bail!(
            "no input: pass a file (oxtail app.ndjson) or pipe NDJSON in (my-app | oxtail)"
        );
    }
    Ok(())
}

fn run_render(file: Option<PathBuf>, lines: Option<usize>, fmt: &FormatArgs) -> Result<()> {
    require_input(&file)?;
    let (rules, _, _) = load_rules(fmt)?;
    let rx = reader::spawn(file, false, None);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut seen = 0usize;
    while lines.is_none_or(|n| seen < n) {
        let Ok(raw) = rx.recv() else { break };
        seen += 1;
        let line = LogLine::parse(raw);
        let text = ui::line_text(&ui::render_line(&line, &rules, false));
        // A closed pipe (e.g. `| head`) just means we're done.
        if writeln!(out, "{text}").is_err() {
            break;
        }
    }
    Ok(())
}

fn run_paths(file: Option<PathBuf>, lines: Option<usize>) -> Result<()> {
    require_input(&file)?;
    let rx = reader::spawn(file, false, None);
    let mut acc = paths::Accumulator::default();
    let mut seen = 0usize;
    while lines.is_none_or(|n| seen < n) {
        let Ok(raw) = rx.recv() else { break };
        seen += 1;
        let line = LogLine::parse(raw);
        acc.add(&line.raw, line.json.as_ref());
    }
    // Tolerate a closed pipe (e.g. piped into `head`).
    let _ = io::stdout().write_all(acc.report().as_bytes());
    Ok(())
}

fn install_skill() -> Result<()> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| anyhow!("could not find home directory (USERPROFILE/HOME unset)"))?;
    let dir = PathBuf::from(home)
        .join(".claude")
        .join("skills")
        .join("oxtail-format");
    fs::create_dir_all(&dir)?;
    let path = dir.join("SKILL.md");
    fs::write(&path, SKILL_MD)?;
    println!("installed skill to {}", path.display());
    Ok(())
}

fn run_tui(cli: Cli) -> Result<()> {
    require_input(&cli.file)?;
    let (rules, config_name, watch_path) = load_rules(&cli.fmt)?;

    let source = cli
        .file
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "stdin".into());
    let rx = reader::spawn(cli.file, cli.follow, cli.rate);

    let mut app = App::new(cli.buffer.max(1), source);
    app.rules = rules;
    app.config_name = config_name;

    // Live-reload the config while the TUI runs.
    let watch = watch_path.map(|p| {
        let seen = mtime(&p);
        (p, seen)
    });

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
