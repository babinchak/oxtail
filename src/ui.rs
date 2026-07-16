use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use serde_json::Value;

use crate::app::{App, LogLine};

/// Stop building spans for a single line past this many chars; the terminal
/// truncates at the right edge anyway and GH Archive payloads can be huge.
const LINE_CHAR_BUDGET: usize = 2048;

pub fn draw(f: &mut Frame, app: &mut App) {
    let [main, status] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(f.area());

    let h = main.height as usize;
    let n = app.lines.len();
    let scroll = if app.follow {
        0
    } else {
        app.scroll.min(n.saturating_sub(h))
    };
    let end = n - scroll;
    let start = end.saturating_sub(h);
    let cursor_idx = n.saturating_sub(1 + app.cursor.min(n.saturating_sub(1)));
    let rows: Vec<Line> = app
        .lines
        .range(start..end)
        .enumerate()
        .map(|(off, l)| {
            let line = render_line(l);
            if !app.follow && start + off == cursor_idx {
                line.bg(Color::DarkGray)
            } else {
                line
            }
        })
        .collect();
    f.render_widget(Paragraph::new(rows), main);

    f.render_widget(status_line(app, start, end), status);

    if let Some(detail) = app.detail.as_mut() {
        detail.render(f);
    }
}

fn status_line(app: &App, start: usize, end: usize) -> Paragraph<'static> {
    let mode = if app.follow {
        Span::styled(" FOLLOW ", Style::new().fg(Color::Black).bg(Color::Green))
    } else {
        Span::styled(" BROWSE ", Style::new().fg(Color::Black).bg(Color::Yellow))
    };
    let mut parts = vec![
        mode,
        Span::raw(format!(
            " {} | {}-{} of {} lines",
            app.source,
            start + 1,
            end,
            app.lines.len()
        )),
    ];
    if app.dropped > 0 {
        parts.push(Span::raw(format!(" (+{} scrolled off)", app.dropped)));
    }
    if app.ended {
        parts.push(Span::styled(
            " | stream ended",
            Style::new().fg(Color::Red),
        ));
    }
    parts.push(Span::styled(
        "  q quit  ↑/↓ move  ⏎ expand  f follow  g/G top/bottom",
        Style::new().add_modifier(Modifier::DIM),
    ));
    Paragraph::new(Line::from(parts)).style(Style::new().bg(Color::DarkGray).fg(Color::White))
}

fn render_line(line: &LogLine) -> Line<'static> {
    match &line.json {
        Some(v) => {
            let mut spans = Vec::new();
            let mut budget = LINE_CHAR_BUDGET;
            json_spans(v, &mut spans, &mut budget);
            Line::from(spans)
        }
        None => Line::from(line.raw.clone()),
    }
}

/// Colorized compact rendering of a JSON value: keys cyan, strings green,
/// numbers yellow, bools magenta, punctuation dim.
fn json_spans(v: &Value, out: &mut Vec<Span<'static>>, budget: &mut usize) {
    if *budget == 0 {
        return;
    }
    let dim = Style::new().add_modifier(Modifier::DIM);
    match v {
        Value::Object(map) => {
            push(out, budget, Span::styled("{", dim));
            for (i, (k, val)) in map.iter().enumerate() {
                if *budget == 0 {
                    return;
                }
                if i > 0 {
                    push(out, budget, Span::styled(",", dim));
                }
                push(out, budget, Span::styled(k.clone(), Color::Cyan));
                push(out, budget, Span::styled(":", dim));
                json_spans(val, out, budget);
            }
            push(out, budget, Span::styled("}", dim));
        }
        Value::Array(items) => {
            push(out, budget, Span::styled("[", dim));
            for (i, val) in items.iter().enumerate() {
                if *budget == 0 {
                    return;
                }
                if i > 0 {
                    push(out, budget, Span::styled(",", dim));
                }
                json_spans(val, out, budget);
            }
            push(out, budget, Span::styled("]", dim));
        }
        Value::String(s) => {
            let escaped = serde_json::to_string(s).unwrap_or_else(|_| format!("{s:?}"));
            push(out, budget, Span::styled(escaped, Color::Green));
        }
        Value::Number(n) => push(out, budget, Span::styled(n.to_string(), Color::Yellow)),
        Value::Bool(b) => push(out, budget, Span::styled(b.to_string(), Color::Magenta)),
        Value::Null => push(out, budget, Span::styled("null", dim)),
    }
}

fn push(out: &mut Vec<Span<'static>>, budget: &mut usize, span: Span<'static>) {
    let len = span.content.chars().count();
    if len >= *budget {
        *budget = 0;
        out.push("…".dim());
    } else {
        *budget -= len;
        out.push(span);
    }
}
