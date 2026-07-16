use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;
use serde_json::Value;

use crate::app::LogLine;

const INDENT: &str = "    ";

/// Expanded, pretty-printed view of a single record. Content is a snapshot
/// taken when opened, so eviction under a firehose can't pull it away.
pub struct Detail {
    lines: Vec<Line<'static>>,
    scroll: usize,
    viewport_h: usize,
    title: String,
}

impl Detail {
    pub fn new(line: &LogLine, global_line_no: u64) -> Self {
        let (lines, kind) = match &line.json {
            Some(v) => (pretty_lines(v), "json"),
            None => (vec![Line::from(line.raw.clone())], "text"),
        };
        Self {
            lines,
            scroll: 0,
            viewport_h: 0,
            title: format!(" line {global_line_no} · {kind} "),
        }
    }

    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll = self.scroll.saturating_sub(amount);
    }

    pub fn scroll_down(&mut self, amount: usize) {
        self.scroll = (self.scroll + amount).min(self.max_scroll());
    }

    pub fn jump_to_top(&mut self) {
        self.scroll = 0;
    }

    pub fn jump_to_bottom(&mut self) {
        self.scroll = self.max_scroll();
    }

    pub fn page(&self) -> usize {
        self.viewport_h.max(1)
    }

    fn max_scroll(&self) -> usize {
        self.lines.len().saturating_sub(self.viewport_h.max(1))
    }

    pub fn render(&mut self, f: &mut Frame) {
        let area = centered(f.area(), 90, 85);
        let block = Block::bordered()
            .title(self.title.clone())
            .title_bottom(Line::from(" esc close  ↑/↓ scroll  g/G top/bottom ").right_aligned())
            .border_style(Style::new().fg(Color::Cyan));
        let inner = block.inner(area);
        self.viewport_h = inner.height as usize;
        self.scroll = self.scroll.min(self.max_scroll());

        let end = (self.scroll + self.viewport_h).min(self.lines.len());
        let visible = self.lines[self.scroll..end].to_vec();

        f.render_widget(Clear, area);
        f.render_widget(Paragraph::new(visible).block(block), area);
    }
}

fn centered(area: Rect, percent_w: u16, percent_h: u16) -> Rect {
    let w = area.width * percent_w / 100;
    let h = area.height * percent_h / 100;
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

/// Pretty-print a JSON value as colorized lines with 4-space indentation.
fn pretty_lines(v: &Value) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    push_value(v, 0, Vec::new(), false, &mut out);
    out
}

fn push_value(
    v: &Value,
    depth: usize,
    mut prefix: Vec<Span<'static>>,
    comma: bool,
    out: &mut Vec<Line<'static>>,
) {
    let dim = Style::new().add_modifier(Modifier::DIM);
    match v {
        Value::Object(map) if !map.is_empty() => {
            prefix.push(Span::styled("{", dim));
            out.push(Line::from(prefix));
            let last = map.len() - 1;
            for (i, (k, val)) in map.iter().enumerate() {
                let key = vec![
                    pad(depth + 1),
                    Span::styled(escape(k), Color::Cyan),
                    Span::styled(": ", dim),
                ];
                push_value(val, depth + 1, key, i != last, out);
            }
            out.push(closer("}", depth, comma));
        }
        Value::Array(items) if !items.is_empty() => {
            prefix.push(Span::styled("[", dim));
            out.push(Line::from(prefix));
            let last = items.len() - 1;
            for (i, val) in items.iter().enumerate() {
                push_value(val, depth + 1, vec![pad(depth + 1)], i != last, out);
            }
            out.push(closer("]", depth, comma));
        }
        _ => {
            prefix.push(scalar_span(v));
            if comma {
                prefix.push(Span::styled(",", dim));
            }
            out.push(Line::from(prefix));
        }
    }
}

fn scalar_span(v: &Value) -> Span<'static> {
    let dim = Style::new().add_modifier(Modifier::DIM);
    match v {
        Value::String(s) => Span::styled(escape(s), Color::Green),
        Value::Number(n) => Span::styled(n.to_string(), Color::Yellow),
        Value::Bool(b) => Span::styled(b.to_string(), Color::Magenta),
        Value::Null => Span::styled("null", dim),
        Value::Object(_) => Span::styled("{}", dim),
        Value::Array(_) => Span::styled("[]", dim),
    }
}

/// JSON-escape a string (quotes included) so control characters and
/// newlines can't corrupt the terminal layout.
fn escape(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("{s:?}"))
}

fn pad(depth: usize) -> Span<'static> {
    Span::raw(INDENT.repeat(depth))
}

fn closer(ch: &'static str, depth: usize, comma: bool) -> Line<'static> {
    let dim = Style::new().add_modifier(Modifier::DIM);
    let mut spans = vec![pad(depth), Span::styled(ch, dim)];
    if comma {
        spans.push(Span::styled(",", dim));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flatten(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn pretty_prints_with_four_space_indent() {
        let v: Value = serde_json::from_str(r#"{"a":[1,2],"b":"x","c":{}}"#).unwrap();
        let text = flatten(&pretty_lines(&v));
        assert_eq!(
            text,
            vec![
                "{",
                "    \"a\": [",
                "        1,",
                "        2",
                "    ],",
                "    \"b\": \"x\",",
                "    \"c\": {}",
                "}",
            ]
        );
    }

    #[test]
    fn escapes_control_characters() {
        let v: Value = serde_json::from_str(r#"{"msg":"two\nlines"}"#).unwrap();
        let text = flatten(&pretty_lines(&v));
        assert_eq!(text[1], "    \"msg\": \"two\\nlines\"");
    }
}
