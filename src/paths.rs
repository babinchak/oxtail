use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

use serde_json::Value;

/// Caps so a pathological stream (e.g. random keys) can't blow up memory.
/// Truncation is always reported, never silent.
const MAX_PATHS: usize = 2_000;
/// Distinct string values tracked per path before it stops counting as a
/// possible discriminant field.
const MAX_DISTINCT: usize = 24;
const EXAMPLE_CHARS: usize = 48;
const EXEMPLAR_CHARS: usize = 160;

/// Dumb, exhaustive structure counter for `--paths`: which dotted paths
/// occur, with what types, how often, plus top-level shape buckets.
/// No inference — deciding what matters is the reader's job.
#[derive(Default)]
pub struct Accumulator {
    json_lines: u64,
    text_lines: u64,
    paths: BTreeMap<String, PathStat>,
    dropped_paths: u64,
    shapes: HashMap<String, Shape>,
}

struct PathStat {
    count: u64,
    types: BTreeSet<&'static str>,
    example: Option<String>,
    /// Distinct string/bool values, while cardinality stays low enough for
    /// the path to be a useful discriminant; None once it overflows.
    values: Option<HashMap<String, u64>>,
}

impl Default for PathStat {
    fn default() -> Self {
        Self {
            count: 0,
            types: BTreeSet::new(),
            example: None,
            values: Some(HashMap::new()),
        }
    }
}

struct Shape {
    count: u64,
    exemplar: String,
}

impl Accumulator {
    pub fn add(&mut self, raw: &str, json: Option<&Value>) {
        let Some(v) = json else {
            self.text_lines += 1;
            return;
        };
        self.json_lines += 1;

        let signature = match v {
            Value::Object(map) => {
                let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
                keys.sort_unstable();
                keys.join(",")
            }
            _ => "(non-object line)".to_string(),
        };
        let shape = self.shapes.entry(signature).or_insert_with(|| Shape {
            count: 0,
            exemplar: truncate(raw, EXEMPLAR_CHARS),
        });
        shape.count += 1;

        let mut path = String::new();
        self.walk(&mut path, v);
    }

    fn walk(&mut self, path: &mut String, v: &Value) {
        if !path.is_empty() {
            self.record(path, v);
        }
        match v {
            Value::Object(map) => {
                for (k, val) in map {
                    let len = path.len();
                    if !path.is_empty() {
                        path.push('.');
                    }
                    path.push_str(k);
                    self.walk(path, val);
                    path.truncate(len);
                }
            }
            // Arrays: walk element 0 only, so paths stay template-addressable
            // (`payload.commits.0.message`) and counts stay per-line.
            Value::Array(items) => {
                if let Some(first) = items.first() {
                    let len = path.len();
                    if !path.is_empty() {
                        path.push('.');
                    }
                    path.push('0');
                    self.walk(path, first);
                    path.truncate(len);
                }
            }
            _ => {}
        }
    }

    fn record(&mut self, path: &str, v: &Value) {
        if !self.paths.contains_key(path) && self.paths.len() >= MAX_PATHS {
            self.dropped_paths += 1;
            return;
        }
        let stat = self.paths.entry(path.to_string()).or_default();
        stat.count += 1;
        stat.types.insert(type_name(v));
        match v {
            Value::String(s) => {
                if stat.example.is_none() && !s.is_empty() {
                    stat.example = Some(truncate(s, EXAMPLE_CHARS));
                }
                track(&mut stat.values, s);
            }
            Value::Bool(b) => track(&mut stat.values, if *b { "true" } else { "false" }),
            Value::Number(n) => {
                if stat.example.is_none() {
                    stat.example = Some(n.to_string());
                }
            }
            _ => {}
        }
    }

    pub fn report(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "input: {} json lines, {} text lines (text excluded below)",
            self.json_lines, self.text_lines
        );

        let mut shapes: Vec<(&String, &Shape)> = self.shapes.iter().collect();
        shapes.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(a.0.cmp(b.0)));
        let _ = writeln!(out, "\ntop-level shapes: {}", shapes.len());
        for (sig, shape) in shapes {
            let _ = writeln!(out, "  {:>8}x  keys: {sig}", shape.count);
            let _ = writeln!(out, "             e.g. {}", shape.exemplar);
        }

        let mut entries: Vec<(&String, &PathStat)> = self.paths.iter().collect();
        entries.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(a.0.cmp(b.0)));
        let width = entries
            .iter()
            .map(|(p, _)| p.chars().count().min(44))
            .max()
            .unwrap_or(0);

        let _ = writeln!(out, "\npaths (by presence):");
        for (path, stat) in entries {
            let types = stat
                .types
                .iter()
                .copied()
                .collect::<Vec<_>>()
                .join("|");
            let pct = stat.count as f64 / self.json_lines.max(1) as f64 * 100.0;
            let _ = writeln!(
                out,
                "  {path:<width$}  {types:<7}  {pct:>5.1}%  {}",
                describe(stat)
            );
        }

        if self.dropped_paths > 0 {
            let _ = writeln!(
                out,
                "\n(!) path cap of {MAX_PATHS} reached: {} occurrences of further paths not shown",
                self.dropped_paths
            );
        }
        out
    }
}

/// Value column: distinct values (a discriminant candidate) or an example.
fn describe(stat: &PathStat) -> String {
    match &stat.values {
        Some(map) if !map.is_empty() => {
            let mut vals: Vec<(&String, &u64)> = map.iter().collect();
            vals.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            // List every tracked value: a reader picking a discriminant
            // needs the complete set, not a teaser.
            let shown = vals
                .iter()
                .map(|(v, c)| format!("{}({c})", truncate(v, EXAMPLE_CHARS)))
                .collect::<Vec<_>>()
                .join(" ");
            format!("{} values: {shown}", vals.len())
        }
        _ => match &stat.example {
            Some(e) => format!("e.g. {e}"),
            None => String::new(),
        },
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn track(values: &mut Option<HashMap<String, u64>>, v: &str) {
    if let Some(map) = values {
        *map.entry(v.to_string()).or_insert(0) += 1;
        if map.len() > MAX_DISTINCT {
            *values = None;
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    let clean: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if clean.chars().count() > max {
        clean.chars().take(max).collect::<String>() + "…"
    } else {
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(lines: &[&str]) -> String {
        let mut acc = Accumulator::default();
        for l in lines {
            let json = serde_json::from_str::<Value>(l)
                .ok()
                .filter(|v| v.is_object() || v.is_array());
            acc.add(l, json.as_ref());
        }
        acc.report()
    }

    #[test]
    fn counts_presence_and_types() {
        let report = feed(&[
            r#"{"type":"A","n":1}"#,
            r#"{"type":"B","n":2,"extra":{"deep":true}}"#,
            "not json",
        ]);
        assert!(report.contains("2 json lines, 1 text lines"), "{report}");
        assert!(report.contains("100.0%"), "{report}");
        assert!(report.contains("extra.deep"), "{report}");
        assert!(report.contains("2 values: A(1) B(1)"), "{report}");
    }

    #[test]
    fn buckets_top_level_shapes() {
        let report = feed(&[
            r#"{"a":1,"b":2}"#,
            r#"{"a":3,"b":4}"#,
            r#"{"c":5}"#,
        ]);
        assert!(report.contains("top-level shapes: 2"), "{report}");
        assert!(report.contains("2x  keys: a,b"), "{report}");
    }

    #[test]
    fn array_paths_use_index_zero() {
        let report = feed(&[r#"{"commits":[{"sha":"abc"},{"sha":"def"}]}"#]);
        assert!(report.contains("commits.0.sha"), "{report}");
    }
}
