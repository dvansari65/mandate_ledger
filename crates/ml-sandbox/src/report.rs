//! The one output shape every command produces.

use crate::Exit;
use serde_json::{Map, Value};
use std::fmt::Write as _;

/// What a command has to say: facts, and tables of rows. Rendered as aligned
/// text for a person, or with `--json` as one object with the same keys for
/// a script. It carries the exit code, because a refusal is a report too — a
/// decision with a context, a stage, a code and a reason — not an error.
pub struct Report {
    exit: Exit,
    items: Vec<Item>,
}

enum Item {
    Fact(&'static str, Value),
    Table {
        label: &'static str,
        columns: &'static [&'static str],
        rows: Vec<Vec<Value>>,
    },
}

impl Report {
    /// A report of something that was allowed, or had nothing to decide.
    pub fn new() -> Self {
        Self {
            exit: Exit::Allowed,
            items: Vec::new(),
        }
    }

    /// A report of a refusal. The facts that follow say what and why.
    pub fn refused() -> Self {
        Self {
            exit: Exit::Refused,
            items: Vec::new(),
        }
    }

    pub fn exit(&self) -> Exit {
        self.exit
    }

    /// Add a fact. Labels are the JSON keys, so keep them short and stable.
    pub fn with(mut self, label: &'static str, value: impl Into<Value>) -> Self {
        self.items.push(Item::Fact(label, value.into()));
        self
    }

    /// Add a table. Every row has one value per column, in column order;
    /// with `--json` it is an array of objects keyed by the column names.
    pub fn table(
        mut self,
        label: &'static str,
        columns: &'static [&'static str],
        rows: Vec<Vec<Value>>,
    ) -> Self {
        debug_assert!(rows.iter().all(|r| r.len() == columns.len()));
        self.items.push(Item::Table {
            label,
            columns,
            rows,
        });
        self
    }

    /// Human-readable text, in the order the items were added.
    pub fn human(&self) -> String {
        let label_width = self
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Fact(l, _) => Some(l.len()),
                Item::Table { .. } => None,
            })
            .max()
            .unwrap_or(0);
        let mut out = String::new();
        for item in &self.items {
            match item {
                Item::Fact(label, value) => {
                    let _ = writeln!(out, "{label:<label_width$}  {}", shown(value));
                }
                Item::Table {
                    label,
                    columns,
                    rows,
                } => {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    if rows.is_empty() {
                        let _ = writeln!(out, "{label}  (none)");
                        continue;
                    }
                    let cells: Vec<Vec<String>> =
                        rows.iter().map(|r| r.iter().map(shown).collect()).collect();
                    let widths: Vec<usize> = columns
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            cells
                                .iter()
                                .map(|r| r[i].len())
                                .chain(std::iter::once(c.len()))
                                .max()
                                .unwrap_or(0)
                        })
                        .collect();
                    let line = |cells: Vec<&str>| {
                        cells
                            .iter()
                            .zip(&widths)
                            .map(|(cell, w)| format!("{cell:<w$}"))
                            .collect::<Vec<_>>()
                            .join("  ")
                            .trim_end()
                            .to_owned()
                    };
                    let _ = writeln!(out, "{}", line(columns.to_vec()));
                    for row in &cells {
                        let _ = writeln!(out, "{}", line(row.iter().map(String::as_str).collect()));
                    }
                }
            }
        }
        out
    }

    /// One pretty-printed JSON object, keys sorted.
    pub fn json(&self) -> String {
        let object: Map<String, Value> = self
            .items
            .iter()
            .map(|item| match item {
                Item::Fact(label, value) => ((*label).to_owned(), value.clone()),
                Item::Table {
                    label,
                    columns,
                    rows,
                } => {
                    let objects = rows
                        .iter()
                        .map(|row| {
                            columns
                                .iter()
                                .zip(row)
                                .map(|(c, v)| ((*c).to_owned(), v.clone()))
                                .collect::<Map<_, _>>()
                                .into()
                        })
                        .collect();
                    ((*label).to_owned(), Value::Array(objects))
                }
            })
            .collect();
        let mut text = serde_json::to_string_pretty(&Value::Object(object))
            .expect("a map of JSON values serializes");
        text.push('\n');
        text
    }
}

/// A value as a person reads it: strings bare, nothing for null.
fn shown(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
