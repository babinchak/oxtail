use std::fs;
use std::path::Path;
use std::str::FromStr;

use ratatui::style::{Color, Style};
use ratatui::text::Span;
use serde::Deserialize;
use serde_json::Value;

use crate::format::{lookup, Template};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    rule: Vec<RawRule>,
    fallback: Option<RawFormat>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    when: RawWhen,
    format: String,
    color: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWhen {
    path: String,
    equals: Option<toml::Value>,
    contains: Option<String>,
    exists: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFormat {
    format: String,
    color: Option<String>,
}

enum Matcher {
    Equals(Value),
    Contains(String),
    Exists,
}

impl Matcher {
    fn matches(&self, found: Option<&Value>) -> bool {
        match self {
            Matcher::Equals(want) => found == Some(want),
            Matcher::Contains(needle) => {
                found.and_then(Value::as_str).is_some_and(|s| s.contains(needle))
            }
            Matcher::Exists => found.is_some(),
        }
    }
}

struct Rule {
    path: String,
    matcher: Matcher,
    template: Template,
    style: Style,
}

/// Compiled display rules: first matching rule formats the line, otherwise
/// the fallback does; no match at all falls through to raw JSON rendering.
#[derive(Default)]
pub struct RuleSet {
    rules: Vec<Rule>,
    fallback: Option<(Template, Style)>,
}

impl RuleSet {
    /// A rule set that formats every JSON line with one template (--format).
    pub fn from_template(template: Template) -> Self {
        Self {
            rules: Vec::new(),
            fallback: Some((template, Style::new())),
        }
    }

    pub fn format_line(&self, v: &Value) -> Option<Vec<Span<'static>>> {
        for rule in &self.rules {
            if rule.matcher.matches(lookup(v, &rule.path)) {
                return Some(rule.template.render_spans(v, rule.style));
            }
        }
        self.fallback
            .as_ref()
            .map(|(t, style)| t.render_spans(v, *style))
    }
}

pub fn load(path: &Path) -> Result<RuleSet, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn parse(text: &str) -> Result<RuleSet, String> {
    let raw: RawConfig = toml::from_str(text).map_err(|e| e.to_string())?;

    let mut rules = Vec::new();
    for (i, r) in raw.rule.iter().enumerate() {
        let n = i + 1;
        let matcher = match (&r.when.equals, &r.when.contains, r.when.exists) {
            (Some(v), None, None) => {
                Matcher::Equals(toml_to_json(v).map_err(|e| format!("rule {n}: {e}"))?)
            }
            (None, Some(s), None) => Matcher::Contains(s.clone()),
            (None, None, Some(true)) => Matcher::Exists,
            _ => {
                return Err(format!(
                    "rule {n}: `when` needs exactly one of `equals`, `contains`, or `exists = true`"
                ));
            }
        };
        rules.push(Rule {
            path: r.when.path.clone(),
            matcher,
            template: Template::parse(&r.format).map_err(|e| format!("rule {n}: {e}"))?,
            style: parse_color(r.color.as_deref()).map_err(|e| format!("rule {n}: {e}"))?,
        });
    }

    let fallback = raw
        .fallback
        .map(|f| -> Result<(Template, Style), String> {
            Ok((
                Template::parse(&f.format).map_err(|e| format!("fallback: {e}"))?,
                parse_color(f.color.as_deref()).map_err(|e| format!("fallback: {e}"))?,
            ))
        })
        .transpose()?;

    Ok(RuleSet { rules, fallback })
}

fn parse_color(name: Option<&str>) -> Result<Style, String> {
    match name {
        None => Ok(Style::new()),
        Some(n) => Color::from_str(n).map(|c| Style::new().fg(c)).map_err(|_| {
            format!(
                "unknown color \"{n}\" (try: red, green, yellow, blue, magenta, cyan, \
                 gray, lightgreen, ..., or \"#rrggbb\")"
            )
        }),
    }
}

fn toml_to_json(v: &toml::Value) -> Result<Value, String> {
    Ok(match v {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => Value::from(*i),
        toml::Value::Float(f) => Value::from(*f),
        toml::Value::Boolean(b) => Value::Bool(*b),
        other => {
            return Err(format!(
                "`equals` must be a string, number, or boolean (got a {})",
                other.type_str()
            ));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
        [[rule]]
        when = { path = "type", equals = "PushEvent" }
        format = "push {repo.name}"
        color = "green"

        [[rule]]
        when = { path = "payload.action", contains = "open" }
        format = "opened #{payload.number}"

        [[rule]]
        when = { path = "error", exists = true }
        format = "ERR {error}"
        color = "red"

        [fallback]
        format = "{type} {repo.name}"
    "#;

    fn fmt(rules: &RuleSet, json: &str) -> Option<String> {
        let v: Value = serde_json::from_str(json).unwrap();
        rules
            .format_line(&v)
            .map(|spans| spans.iter().map(|s| s.content.as_ref()).collect())
    }

    #[test]
    fn first_matching_rule_wins() {
        let rules = parse(CONFIG).unwrap();
        assert_eq!(
            fmt(&rules, r#"{"type":"PushEvent","repo":{"name":"a/b"}}"#).unwrap(),
            "push a/b"
        );
        assert_eq!(
            fmt(&rules, r#"{"payload":{"action":"opened","number":7}}"#).unwrap(),
            "opened #7"
        );
        assert_eq!(fmt(&rules, r#"{"error":"boom"}"#).unwrap(), "ERR boom");
    }

    #[test]
    fn fallback_applies_when_no_rule_matches() {
        let rules = parse(CONFIG).unwrap();
        assert_eq!(
            fmt(&rules, r#"{"type":"ForkEvent","repo":{"name":"x/y"}}"#).unwrap(),
            "ForkEvent x/y"
        );
    }

    #[test]
    fn no_fallback_means_none() {
        let rules = parse("").unwrap();
        assert_eq!(fmt(&rules, r#"{"a":1}"#), None);
    }

    #[test]
    fn rejects_ambiguous_matcher() {
        let err = parse(
            r#"
            [[rule]]
            when = { path = "a", equals = "x", contains = "y" }
            format = "z"
            "#,
        )
        .err()
        .unwrap();
        assert!(err.contains("exactly one of"), "{err}");
    }

    #[test]
    fn rejects_unknown_color_with_hint() {
        let err = parse(
            r#"
            [[rule]]
            when = { path = "a", exists = true }
            format = "z"
            color = "greeen"
            "#,
        )
        .err()
        .unwrap();
        assert!(err.contains("rule 1") && err.contains("greeen"), "{err}");
    }

    #[test]
    fn rejects_unknown_config_keys() {
        assert!(parse("[[rule]]\nwhen = { path = \"a\", exists = true }\nformat = \"x\"\ncolour = \"red\"").is_err());
    }
}
