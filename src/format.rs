use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use serde_json::Value;

/// Max chars when a template field resolves to a whole object/array.
const CONTAINER_BUDGET: usize = 120;

enum Segment {
    Literal(String),
    Field(String),
}

/// A line format like `"{created_at} push {repo.name} by {actor.login}"`.
/// `{a.b.c}` placeholders are dotted paths into the JSON record (array
/// indices are numeric segments, e.g. `{payload.commits.0.message}`).
/// `{{` and `}}` render literal braces.
pub struct Template {
    segments: Vec<Segment>,
}

impl Template {
    pub fn parse(input: &str) -> Result<Self, String> {
        let mut segments = Vec::new();
        let mut lit = String::new();
        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '{' if chars.peek() == Some(&'{') => {
                    chars.next();
                    lit.push('{');
                }
                '}' if chars.peek() == Some(&'}') => {
                    chars.next();
                    lit.push('}');
                }
                '{' => {
                    let mut path = String::new();
                    loop {
                        match chars.next() {
                            Some('}') => break,
                            Some(ch) => path.push(ch),
                            None => {
                                return Err(format!(
                                    "unclosed '{{' in template (started before end: \"{{{path}\")"
                                ));
                            }
                        }
                    }
                    if path.is_empty() {
                        return Err("empty {} placeholder in template".into());
                    }
                    if !lit.is_empty() {
                        segments.push(Segment::Literal(std::mem::take(&mut lit)));
                    }
                    segments.push(Segment::Field(path));
                }
                '}' => {
                    return Err("unmatched '}' in template (use \"}}\" for a literal brace)".into());
                }
                _ => lit.push(c),
            }
        }
        if !lit.is_empty() {
            segments.push(Segment::Literal(lit));
        }
        Ok(Self { segments })
    }

    /// Render against one record: literals dim, field values in `value_style`,
    /// missing fields as a dim `-`.
    pub fn render_spans(&self, v: &Value, value_style: Style) -> Vec<Span<'static>> {
        let dim = Style::new().add_modifier(Modifier::DIM);
        self.segments
            .iter()
            .map(|seg| match seg {
                Segment::Literal(s) => Span::styled(s.clone(), dim),
                Segment::Field(path) => match lookup(v, path) {
                    Some(val) => Span::styled(display_value(val), value_style),
                    None => Span::styled("-", dim),
                },
            })
            .collect()
    }
}

/// Resolve a dotted path (`payload.issue.title`, `payload.commits.0.sha`).
pub fn lookup<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for part in path.split('.') {
        cur = match cur {
            Value::Object(map) => map.get(part)?,
            Value::Array(items) => items.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

fn display_value(v: &Value) -> String {
    match v {
        // Strings render unquoted; control chars would corrupt the layout.
        Value::String(s) => s
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "-".into(),
        container => {
            let s = serde_json::to_string(container).unwrap_or_default();
            if s.chars().count() > CONTAINER_BUDGET {
                s.chars().take(CONTAINER_BUDGET).collect::<String>() + "…"
            } else {
                s
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(template: &str, json: &str) -> String {
        let t = Template::parse(template).unwrap();
        let v: Value = serde_json::from_str(json).unwrap();
        t.render_spans(&v, Style::new())
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn renders_fields_and_literals() {
        let out = render(
            "{created_at} push {repo.name} by {actor.login}",
            r#"{"created_at":"2015-01-01","repo":{"name":"a/b"},"actor":{"login":"ann"}}"#,
        );
        assert_eq!(out, "2015-01-01 push a/b by ann");
    }

    #[test]
    fn missing_field_renders_dash() {
        assert_eq!(render("x {nope.deep} y", r#"{"a":1}"#), "x - y");
    }

    #[test]
    fn array_index_paths() {
        assert_eq!(
            render("{commits.0.sha}", r#"{"commits":[{"sha":"abc"}]}"#),
            "abc"
        );
    }

    #[test]
    fn escaped_braces() {
        assert_eq!(render("a {{literal}} {n}", r#"{"n":5}"#), "a {literal} 5");
    }

    #[test]
    fn parse_errors() {
        assert!(Template::parse("{unclosed").is_err());
        assert!(Template::parse("{}").is_err());
        assert!(Template::parse("stray } brace").is_err());
    }

    #[test]
    fn control_chars_in_values_are_sanitized() {
        assert_eq!(render("{msg}", r#"{"msg":"a\nb"}"#), "a b");
    }
}
