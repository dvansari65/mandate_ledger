//! The one output shape every command produces.

use serde_json::{Map, Value};
use std::fmt::Write as _;

/// An ordered list of facts about what a command did. Rendered as aligned
/// `label  value` lines, or with `--json` as one object with the same keys.
pub struct Report {
    fields: Vec<(&'static str, Value)>,
}

impl Report {
    pub fn new() -> Self {
        Self { fields: Vec::new() }
    }

    /// Add a fact. Labels are the JSON keys, so keep them short and stable.
    pub fn with(mut self, label: &'static str, value: impl Into<Value>) -> Self {
        self.fields.push((label, value.into()));
        self
    }

    /// Human-readable lines, in the order the facts were added.
    pub fn human(&self) -> String {
        let width = self.fields.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
        let mut out = String::new();
        for (label, value) in &self.fields {
            let shown = match value {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let _ = writeln!(out, "{label:<width$}  {shown}");
        }
        out
    }

    /// One pretty-printed JSON object, keys sorted.
    pub fn json(&self) -> String {
        let object: Map<String, Value> = self
            .fields
            .iter()
            .map(|(l, v)| ((*l).to_owned(), v.clone()))
            .collect();
        let mut text = serde_json::to_string_pretty(&Value::Object(object))
            .expect("a map of JSON values serializes");
        text.push('\n');
        text
    }
}
