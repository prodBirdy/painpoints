use crate::jev::{level_of, model, Record, DIMENSIONS, PAIN_THRESHOLD};
use crate::report::{self, Report};
use crate::run;
use crate::scan::Config;
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

const PROTOCOL: &str = "2025-06-18";

pub fn round2_value(value: f32) -> f64 {
    ((value as f64) * 100.0).round() / 100.0
}

pub fn findings(record: &Record) -> Vec<Value> {
    let mut found: Vec<Value> = DIMENSIONS
        .iter()
        .map(|dimension| (dimension, record.scores.get(dimension.key)))
        .filter(|(_, score)| *score >= PAIN_THRESHOLD)
        .map(|(dimension, score)| {
            json!({
                "dimension": dimension.key,
                "label": dimension.label,
                "score": round2_value(score),
                "level": level_of(score),
                "description": dimension.levels[level_of(score)],
                "source": dimension.source,
                "url": dimension.url,
            })
        })
        .collect();
    found.sort_by(|a, b| {
        b["score"]
            .as_f64()
            .unwrap_or(0.)
            .total_cmp(&a["score"].as_f64().unwrap_or(0.))
    });
    found
}

pub fn file_result(record: &Record, cached: bool) -> Value {
    json!({
        "path": record.path,
        "lines": record.lines,
        "role": record.role,
        "role_confidence": round2_value(record.role_confidence),
        "scores": record.scores,
        "worst_dimension": record.worst_dimension,
        "worst_score": round2_value(record.worst_score),
        "total_score": round2_value(record.total_score),
        "needs_review": record.needs_review,
        "is_pain_point": record.worst_score >= PAIN_THRESHOLD,
        "pain_threshold": PAIN_THRESHOLD,
        "findings": findings(record),
        "cached": cached,
        "model": model(),
    })
}

fn out_dir(root: &Path) -> PathBuf {
    root.join(".painpoints")
}

fn classify_file(args: &Value) -> Result<Value> {
    let Some(path) = args.get("path").and_then(Value::as_str) else {
        bail!("path is required");
    };
    let file = crate::scan::absolute(PathBuf::from(path));
    if !file.is_file() {
        bail!("{path} is not a file");
    }
    let config = Config::single(file);
    let root = config.root.clone();
    let dir = out_dir(&root);
    let refresh = args.get("refresh").and_then(Value::as_bool).unwrap_or(false);
    let cache = if refresh {
        Default::default()
    } else {
        report::load(&dir)
    };

    let outcome = run::blocking(config, cache, |_, _| {})?;
    if let Some((path, error)) = outcome.failures.first() {
        bail!("{path}: {error}");
    }
    let Some(record) = outcome.records.first() else {
        bail!("{path} is not a source file painpoints recognises");
    };

    report::merge_write(&dir, &root, &outcome.records, outcome.usage)?;
    let mut result = file_result(record, outcome.cached == 1);
    result["tokens"] = json!(outcome.usage);
    Ok(result)
}

fn classify_repo(args: &Value) -> Result<Value> {
    let root = crate::scan::absolute(
        args.get("root")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(crate::scan::default_root),
    );
    if !root.is_dir() {
        bail!("{} is not a directory", root.display());
    }

    let mut config = Config::new(root.clone());
    if let Some(include) = args.get("include").and_then(Value::as_array) {
        config.includes = include
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
    }
    config.limit = args.get("limit").and_then(Value::as_u64).unwrap_or(0) as usize;

    let dir = out_dir(&root);
    let refresh = args.get("refresh").and_then(Value::as_bool).unwrap_or(false);
    let cache = if refresh {
        Default::default()
    } else {
        report::load(&dir)
    };

    let outcome = run::blocking(config, cache, |_, _| {})?;
    let report = Report {
        records: outcome.records,
        failures: outcome.failures,
        usage: outcome.usage,
        root: root.clone(),
    };
    let (json_path, md_path) = report.write(&dir)?;

    let top = args.get("top").and_then(Value::as_u64).unwrap_or(20) as usize;
    let mut summary = report.summary();
    summary["root"] = json!(root.to_string_lossy());
    summary["cached"] = json!(outcome.cached);
    summary["report"] = json!({
        "json": json_path.to_string_lossy(),
        "markdown": md_path.to_string_lossy(),
    });
    summary["top"] = json!(report
        .ranked()
        .into_iter()
        .take(top)
        .map(|record| {
            json!({
                "path": record.path,
                "role": record.role,
                "worst_dimension": record.worst_dimension,
                "worst_score": round2_value(record.worst_score),
                "scores": record.scores,
                "findings": findings(record),
            })
        })
        .collect::<Vec<_>>());
    Ok(summary)
}

fn tools() -> Value {
    json!([
        {
            "name": "painpoints_file",
            "title": "Classify one file",
            "description": "Score a single source file on six architectural pain dimensions and return the atomic result: every score, the worst one, and a finding per dimension at 2.0 or above carrying the level description and the published standard it was judged against. Reuses the saved report as a cache, so an unchanged file costs no tokens.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the source file."
                    },
                    "refresh": {
                        "type": "boolean",
                        "description": "Reclassify even if the cached digest still matches."
                    }
                },
                "required": ["path"]
            }
        },
        {
            "name": "painpoints_repo",
            "title": "Classify a repository",
            "description": "Score every source file in a repository and return the distribution plus the worst files, each with its findings. Writes the full JSON and Markdown report next to the repository. Only files whose contents changed cost tokens.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "root": {
                        "type": "string",
                        "description": "Repository root. Defaults to PAINPOINTS_ROOT or the working directory."
                    },
                    "include": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Only walk these subdirectories."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Stop after this many files."
                    },
                    "top": {
                        "type": "integer",
                        "description": "How many of the worst files to return inline. Defaults to 20."
                    },
                    "refresh": {
                        "type": "boolean",
                        "description": "Reclassify everything instead of reusing the saved report."
                    }
                }
            }
        }
    ])
}

fn ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn failed(id: Value, code: i32, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn call(id: Value, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let outcome = match name {
        "painpoints_file" => classify_file(&args),
        "painpoints_repo" => classify_repo(&args),
        other => Err(anyhow::anyhow!("unknown tool {other}")),
    };
    match outcome {
        Ok(value) => ok(
            id,
            json!({
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default() }],
                "structuredContent": value,
                "isError": false,
            }),
        ),
        Err(err) => ok(
            id,
            json!({
                "content": [{ "type": "text", "text": err.to_string() }],
                "isError": true,
            }),
        ),
    }
}

pub fn serve() -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            continue;
        };
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));

        let response = match (method, id) {
            ("initialize", Some(id)) => Some(ok(
                id,
                json!({
                    "protocolVersion": PROTOCOL,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": {
                        "name": env!("CARGO_PKG_NAME"),
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                }),
            )),
            ("ping", Some(id)) => Some(ok(id, json!({}))),
            ("tools/list", Some(id)) => Some(ok(id, json!({ "tools": tools() }))),
            ("tools/call", Some(id)) => Some(call(id, &params)),
            (_, Some(id)) => Some(failed(id, -32601, format!("unknown method {method}"))),
            (_, None) => None,
        };

        if let Some(response) = response {
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jev::Scores;

    fn record() -> Record {
        Record {
            path: "server/db.ts".into(),
            lines: 120,
            role: "data-access".into(),
            role_confidence: 0.93,
            scores: Scores {
                boundary_leak: 1.1,
                complexity: 0.6,
                data_access_cost: 2.7,
                failure_handling: 2.2,
                interaction_cost: 0.0,
                trust_boundary_risk: 0.4,
            },
            worst_dimension: "data_access_cost".into(),
            worst_score: 2.7,
            total_score: 7.0,
            needs_review: false,
            digest: "abc".into(),
        }
    }

    #[test]
    fn findings_carry_only_real_pain_worst_first() {
        let found = findings(&record());
        assert_eq!(found.len(), 2);
        assert_eq!(found[0]["dimension"], "data_access_cost");
        assert_eq!(found[1]["dimension"], "failure_handling");
        assert_eq!(found[0]["level"], 3);
        assert!(found[0]["description"].as_str().unwrap().contains("loop"));
        assert!(found[0]["url"].as_str().unwrap().starts_with("https://"));
    }

    #[test]
    fn a_file_result_is_self_contained() {
        let value = file_result(&record(), true);
        assert_eq!(value["path"], "server/db.ts");
        assert_eq!(value["is_pain_point"], true);
        assert_eq!(value["cached"], true);
        assert_eq!(value["scores"]["data_access_cost"], 2.7);
    }

    #[test]
    fn every_tool_declares_a_schema() {
        let listed = tools();
        let listed = listed.as_array().expect("array");
        assert_eq!(listed.len(), 2);
        for tool in listed {
            assert!(tool["name"].as_str().is_some());
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert!(tool["description"].as_str().unwrap().len() > 40);
        }
        assert_eq!(listed[0]["inputSchema"]["required"][0], "path");
    }

    #[test]
    fn an_unknown_tool_is_an_error_result_not_a_protocol_error() {
        let response = call(json!(7), &json!({ "name": "nope", "arguments": {} }));
        assert_eq!(response["id"], 7);
        assert_eq!(response["result"]["isError"], true);
        assert!(response["error"].is_null());
    }
}
