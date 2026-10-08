//! Keep JSON- and TOML-backed MEASURED rows in FACTS.md tied to committed files.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

fn measured_rows(facts: &str) -> Vec<(String, String, String)> {
    let mut rows = Vec::new();
    for line in facts.lines() {
        if !line.starts_with('|') {
            continue;
        }
        let cells: Vec<&str> = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        if cells.len() == 4 && cells[3] == "MEASURED" && cells[0] != "Claim" {
            rows.push((
                cells[0].to_owned(),
                cells[1].to_owned(),
                cells[2].to_owned(),
            ));
        }
    }
    rows
}

fn keyed_source(source: &str) -> Option<(&str, &str, &str)> {
    for kind in ["json", "toml"] {
        let prefix = format!("{kind}:");
        let Some(start) = source.find(&prefix) else {
            continue;
        };
        let rest = source[start + prefix.len()..].trim_matches('`');
        let (path, key) = rest.split_once('#')?;
        let path = path.trim();
        let key = key
            .split(|ch: char| ch.is_whitespace() || matches!(ch, '`' | ';' | '|' | ')' | ','))
            .next()
            .unwrap_or("");
        if key.is_empty() {
            continue;
        }
        if kind == "json" && path.ends_with(".json") {
            return Some(("json", path, key));
        }
        if kind == "toml" && path.ends_with(".toml") {
            return Some(("toml", path, key));
        }
    }
    None
}

fn lookup<'a>(value: &'a Value, key: &str) -> Result<&'a Value, String> {
    let mut current = value;
    for part in key.split('.') {
        let Value::Object(map) = current else {
            return Err(format!("key {key:?} does not exist"));
        };
        current = map
            .get(part)
            .ok_or_else(|| format!("key {key:?} does not exist"))?;
    }
    Ok(current)
}

/// Parse the subset of TOML used by FACTS.md sources: tables and quoted or numeric keys.
fn parse_toml_tables(text: &str) -> Value {
    let mut root = Map::new();
    let mut current_path: Vec<String> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[')
            && let Some(header) = header.strip_suffix(']')
            && !header.starts_with('[')
        {
            current_path = header
                .split('.')
                .map(|part| part.trim().to_owned())
                .collect();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let parsed = parse_toml_scalar(value.trim());
        let mut map = &mut root;
        for part in &current_path {
            let entry = map
                .entry(part.clone())
                .or_insert_with(|| Value::Object(Map::new()));
            map = entry.as_object_mut().expect("TOML table");
        }
        map.insert(key.to_owned(), parsed);
    }
    Value::Object(root)
}

fn parse_toml_scalar(value: &str) -> Value {
    let value = value.split('#').next().unwrap_or(value).trim();
    if let Some(body) = value.strip_prefix('"')
        && let Some(body) = body.strip_suffix('"')
    {
        return Value::String(body.to_owned());
    }
    if value == "true" {
        return Value::Bool(true);
    }
    if value == "false" {
        return Value::Bool(false);
    }
    if let Ok(number) = value.parse::<i64>() {
        return serde_json::json!(number);
    }
    if let Ok(number) = value.parse::<f64>() {
        return serde_json::json!(number);
    }
    Value::String(value.to_owned())
}

fn assert_subset(actual: &Value, expected: &Value, claim: &str) {
    if let Value::Object(expected_map) = expected {
        let Value::Object(actual_map) = actual else {
            panic!("{claim}: source value is not an object");
        };
        for (key, value) in expected_map {
            let child = actual_map
                .get(key)
                .unwrap_or_else(|| panic!("{claim}: source object has no {key:?}"));
            assert_subset(child, value, &format!("{claim}.{key}"));
        }
        return;
    }
    assert_eq!(
        actual, expected,
        "{claim}: FACTS.md has {expected}, source has {actual}"
    );
}

#[test]
fn measured_json_and_toml_sources_match_fact_sheet() {
    let root = workspace_root();
    let facts = fs::read_to_string(root.join("FACTS.md")).expect("FACTS.md");
    let mut checked = 0usize;
    for (claim, exact, source) in measured_rows(&facts) {
        let Some((kind, path, key)) = keyed_source(&source) else {
            continue;
        };
        let file = root.join(path);
        assert!(
            file.is_file(),
            "{claim}: source does not exist: {}",
            file.display()
        );
        assert!(
            exact.starts_with('`') && exact.ends_with('`') && exact.len() >= 2,
            "{claim}: exact value must be one JSON code span"
        );
        let expected: Value = serde_json::from_str(&exact[1..exact.len() - 1])
            .unwrap_or_else(|error| panic!("{claim}: exact value is not JSON: {error}"));
        let parsed = if kind == "json" {
            serde_json::from_str(&fs::read_to_string(&file).expect(path))
                .unwrap_or_else(|error| panic!("{claim}: invalid JSON: {error}"))
        } else {
            parse_toml_tables(&fs::read_to_string(&file).expect(path))
        };
        let actual = lookup(&parsed, key).unwrap_or_else(|error| panic!("{claim}: {error}"));
        assert_subset(actual, &expected, &claim);
        checked += 1;
    }
    assert!(
        checked >= 20,
        "expected at least 20 JSON- or TOML-backed measured rows, checked {checked}"
    );
}
