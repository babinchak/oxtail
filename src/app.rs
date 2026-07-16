use std::collections::VecDeque;

use serde_json::Value;

use crate::config::RuleSet;
use crate::detail::Detail;

/// One log line: the raw text, plus its parsed form when it is NDJSON.
pub struct LogLine {
    pub raw: String,
    pub json: Option<Value>,
}

impl LogLine {
    pub fn parse(raw: String) -> Self {
        // Only objects/arrays count as structured; a bare `42` or `"hi"` line
        // is valid JSON but should still read as plain text.
        let json = serde_json::from_str::<Value>(&raw)
            .ok()
            .filter(|v| v.is_object() || v.is_array());
        Self { raw, json }
    }
}

pub struct App {
    pub lines: VecDeque<LogLine>,
    /// Max lines kept in memory; oldest are evicted past this.
    pub cap: usize,
    /// Lines evicted from the front of the buffer so far.
    pub dropped: u64,
    /// When true, the view is pinned to the newest line.
    pub follow: bool,
    /// Offset of the bottom visible line, in lines from the newest.
    pub scroll: usize,
    /// Offset of the selected line, in lines from the newest (browse mode).
    pub cursor: usize,
    /// Height of the log viewport, updated each frame before input handling.
    pub viewport_h: usize,
    /// Expanded view of one record, when open.
    pub detail: Option<Detail>,
    /// Display rules from --format or the config file.
    pub rules: RuleSet,
    /// Show raw JSON even when rules exist (toggled with `r`).
    pub raw_mode: bool,
    /// Where the rules came from, for the status bar (file name or --format).
    pub config_name: Option<String>,
    /// Error from the last live config reload; the old rules stay active.
    pub config_error: Option<String>,
    /// The input stream reached EOF (reader thread hung up).
    pub ended: bool,
    /// Display name of the input source for the status bar.
    pub source: String,
    pub quit: bool,
}

impl App {
    pub fn new(cap: usize, source: String) -> Self {
        Self {
            lines: VecDeque::new(),
            cap,
            dropped: 0,
            follow: true,
            scroll: 0,
            cursor: 0,
            viewport_h: 0,
            detail: None,
            rules: RuleSet::default(),
            raw_mode: false,
            config_name: None,
            config_error: None,
            ended: false,
            source,
            quit: false,
        }
    }

    pub fn push(&mut self, raw: String) {
        self.lines.push_back(LogLine::parse(raw));
        if self.lines.len() > self.cap {
            self.lines.pop_front();
            self.dropped += 1;
        }
        // While browsing, grow the from-the-bottom offsets so the view and
        // selection stay anchored on the same content as new lines arrive.
        if !self.follow {
            self.scroll = (self.scroll + 1).min(self.max_scroll());
            self.cursor = (self.cursor + 1).min(self.lines.len() - 1);
        }
    }

    fn max_scroll(&self) -> usize {
        self.lines.len().saturating_sub(self.viewport_h.max(1))
    }

    /// The line under the cursor (the newest line while following).
    pub fn selected(&self) -> Option<&LogLine> {
        let n = self.lines.len();
        self.lines.get(n.checked_sub(1 + self.cursor.min(n))?)
    }

    pub fn open_detail(&mut self) {
        let n = self.lines.len();
        let Some(line) = self.selected() else { return };
        let idx = n - 1 - self.cursor.min(n - 1);
        let global = self.dropped + idx as u64 + 1;
        self.detail = Some(Detail::new(line, global));
    }

    pub fn scroll_up(&mut self, amount: usize) {
        self.follow = false;
        let max = self.lines.len().saturating_sub(1);
        self.cursor = (self.cursor + amount).min(max);
        self.ensure_cursor_visible();
    }

    pub fn scroll_down(&mut self, amount: usize) {
        self.cursor = self.cursor.saturating_sub(amount);
        // Reaching the bottom re-engages follow mode.
        if self.cursor == 0 {
            self.follow = true;
        }
        self.ensure_cursor_visible();
    }

    pub fn scroll_to_top(&mut self) {
        self.follow = false;
        self.cursor = self.lines.len().saturating_sub(1);
        self.ensure_cursor_visible();
    }

    pub fn resume_follow(&mut self) {
        self.follow = true;
        self.cursor = 0;
        self.scroll = 0;
    }

    /// Keep the cursor inside the visible window [scroll, scroll + height).
    fn ensure_cursor_visible(&mut self) {
        let h = self.viewport_h.max(1);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + h {
            self.scroll = self.cursor + 1 - h;
        }
        self.scroll = self.scroll.min(self.max_scroll());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_line_is_json() {
        let l = LogLine::parse(r#"{"type":"PushEvent","id":1}"#.into());
        assert!(l.json.is_some());
    }

    #[test]
    fn plain_text_is_not_json() {
        let l = LogLine::parse("plain old log line".into());
        assert!(l.json.is_none());
    }

    #[test]
    fn bare_scalar_json_is_treated_as_text() {
        assert!(LogLine::parse("42".into()).json.is_none());
        assert!(LogLine::parse(r#""hello""#.into()).json.is_none());
    }

    #[test]
    fn buffer_evicts_past_cap() {
        let mut app = App::new(3, "test".into());
        for i in 0..5 {
            app.push(format!("line {i}"));
        }
        assert_eq!(app.lines.len(), 3);
        assert_eq!(app.dropped, 2);
        assert_eq!(app.lines[0].raw, "line 2");
    }

    #[test]
    fn selection_stays_anchored_as_lines_arrive() {
        let mut app = App::new(100, "test".into());
        app.viewport_h = 10;
        for i in 0..20 {
            app.push(format!("line {i}"));
        }
        app.scroll_up(5);
        assert!(!app.follow);
        let before = app.selected().unwrap().raw.clone();
        app.push("newcomer".into());
        assert_eq!(app.selected().unwrap().raw, before);
    }

    #[test]
    fn scrolling_to_bottom_resumes_follow() {
        let mut app = App::new(100, "test".into());
        app.viewport_h = 10;
        for i in 0..20 {
            app.push(format!("line {i}"));
        }
        app.scroll_up(3);
        app.scroll_down(3);
        assert!(app.follow);
    }
}
