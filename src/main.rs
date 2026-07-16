mod app;
mod detail;
mod reader;
mod ui;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::app::App;

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
}

fn main() -> Result<()> {
    let args = Args::parse();
    let source = args
        .file
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "stdin".into());
    let rx = reader::spawn(args.file, args.follow, args.rate);
    let mut app = App::new(args.buffer.max(1), source);

    let terminal = ratatui::init();
    let result = run(terminal, &mut app, rx);
    ratatui::restore();
    result
}

/// Max lines ingested per frame so a firehose can't starve the render loop.
const DRAIN_PER_FRAME: usize = 5_000;

fn run(mut terminal: DefaultTerminal, app: &mut App, rx: Receiver<String>) -> Result<()> {
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
        KeyCode::Up | KeyCode::Char('k') => app.scroll_up(1),
        KeyCode::Down | KeyCode::Char('j') => app.scroll_down(1),
        KeyCode::PageUp => app.scroll_up(page),
        KeyCode::PageDown => app.scroll_down(page),
        KeyCode::Home | KeyCode::Char('g') => app.scroll_to_top(),
        KeyCode::End | KeyCode::Char('G') | KeyCode::Char('f') => app.resume_follow(),
        _ => {}
    }
}
