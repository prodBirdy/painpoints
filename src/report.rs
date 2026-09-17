use crate::jev::{Record, Usage, DIMENSIONS, MODEL, PAIN_THRESHOLD};
use anyhow::Result;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const TOP_FILES: usize = 30;
const PER_DIMENSION: usize = 15;

pub fn load(dir: &Path) -> HashMap<String, Record> {
    let Ok(bytes) = std::fs::read(dir.join("architecture-pain.json")) else {
        return HashMap::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return HashMap::new();
    };
    serde_json::from_value::<Vec<Record>>(value["files"].clone())
        .unwrap_or_default()
        .into_iter()
        .map(|record| (record.path.clone(), record))
        .collect()
}

pub struct Report {
    pub records: Vec<Record>,
    pub failures: Vec<(String, String)>,
    pub usage: Usage,
    pub root: PathBuf,
}

impl Report {
    fn ranked(&self) -> Vec<&Record> {
        let mut sorted: Vec<&Record> = self.records.iter().collect();
        sorted.sort_by(|a, b| a.rank_key().cmp(&b.rank_key()));
        sorted
    }

    fn pain_points(&self) -> usize {
        self.records
            .iter()
            .filter(|r| r.worst_score >= PAIN_THRESHOLD)
            .count()
    }

    fn tally<'a>(&'a self, key: impl Fn(&'a Record) -> &'a str) -> BTreeMap<&'a str, usize> {
        let mut counts = BTreeMap::new();
        for record in &self.records {
            *counts.entry(key(record)).or_insert(0) += 1;
        }
        counts
    }

    fn json(&self) -> serde_json::Value {
        let by_dimension: BTreeMap<&str, usize> = DIMENSIONS
            .iter()
            .map(|d| {
                let hits = self
                    .records
                    .iter()
                    .filter(|r| r.scores.get(d.key) >= PAIN_THRESHOLD)
                    .count();
                (d.key, hits)
            })
            .collect();

        json!({
            "model": MODEL,
            "root": self.root.to_string_lossy(),
            "scale": "0 healthy to 3 painful, scored per file; a file is a pain point in a dimension at 2.0 or above",
            "dimensions": DIMENSIONS.iter().map(|d| json!({
                "key": d.key,
                "label": d.label,
                "source": d.source,
                "url": d.url,
            })).collect::<Vec<_>>(),
            "summary": {
                "files_classified": self.records.len(),
                "pain_points": self.pain_points(),
                "needs_review": self.records.iter().filter(|r| r.needs_review).count(),
                "by_dimension": by_dimension,
                "by_role": self.tally(|r| r.role.as_str()),
                "usage": self.usage,
            },
            "files": self.ranked(),
            "failures": self.failures.iter().map(|(path, error)| json!({
                "path": path,
                "error": error,
            })).collect::<Vec<_>>(),
        })
    }

    fn markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Architecture pain points\n");
        let _ = writeln!(
            out,
            "`{}` classified {} files under `{}` with TypeSafe {}.\n",
            env!("CARGO_PKG_NAME"),
            self.records.len(),
            self.root.to_string_lossy(),
            MODEL
        );
        let _ = writeln!(
            out,
            "Every file is scored 0 (healthy) to 3 (painful) on six dimensions. \
             A file counts as a pain point in a dimension at {PAIN_THRESHOLD:.1} or above. \
             Files are ranked by their worst dimension first, then by how many dimensions hurt. \
             The scores are model judgments over the first 8000 characters of each file: \
             treat them as a reading order, not as proof.\n"
        );

        let _ = writeln!(out, "## Dimensions\n");
        let _ = writeln!(out, "| dimension | files at {PAIN_THRESHOLD:.1} or above | standard |");
        let _ = writeln!(out, "| --- | --- | --- |");
        for dimension in DIMENSIONS {
            let hits = self
                .records
                .iter()
                .filter(|r| r.scores.get(dimension.key) >= PAIN_THRESHOLD)
                .count();
            let _ = writeln!(
                out,
                "| `{}` | {} | [{}]({}) |",
                dimension.key, hits, dimension.source, dimension.url
            );
        }

        let ranked = self.ranked();
        let _ = writeln!(out, "\n## Worst {TOP_FILES} files\n");
        let columns: Vec<&str> = DIMENSIONS.iter().map(|d| d.short).collect();
        let _ = writeln!(
            out,
            "| file | role | worst | score | {} |",
            columns.join(" | ")
        );
        let _ = writeln!(
            out,
            "|{}|",
            " --- |".repeat(4 + columns.len())
        );
        for record in ranked.iter().take(TOP_FILES) {
            let s = &record.scores;
            let _ = writeln!(
                out,
                "| `{}` | {} | {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} |",
                record.path,
                record.role,
                record.worst_dimension,
                record.worst_score,
                s.boundary_leak,
                s.complexity,
                s.data_access_cost,
                s.failure_handling,
                s.interaction_cost,
                s.trust_boundary_risk,
            );
        }

        for dimension in DIMENSIONS {
            let mut hits: Vec<&&Record> = ranked
                .iter()
                .filter(|r| r.scores.get(dimension.key) >= PAIN_THRESHOLD)
                .collect();
            hits.sort_by(|a, b| {
                b.scores
                    .get(dimension.key)
                    .total_cmp(&a.scores.get(dimension.key))
            });
            if hits.is_empty() {
                continue;
            }
            let _ = writeln!(out, "\n## {}\n", dimension.label);
            let _ = writeln!(out, "Standard: [{}]({})\n", dimension.source, dimension.url);
            for record in hits.iter().take(PER_DIMENSION) {
                let _ = writeln!(
                    out,
                    "- `{}` {:.1} ({})",
                    record.path,
                    record.scores.get(dimension.key),
                    record.role
                );
            }
            if hits.len() > PER_DIMENSION {
                let _ = writeln!(out, "- and {} more in the JSON report", hits.len() - PER_DIMENSION);
            }
        }

        let unsure: Vec<&&Record> = ranked.iter().filter(|r| r.needs_review).collect();
        if !unsure.is_empty() {
            let _ = writeln!(out, "\n## Low confidence\n");
            let _ = writeln!(
                out,
                "The model was unsure about these; read the file before acting on its scores.\n"
            );
            for record in unsure.iter().take(PER_DIMENSION) {
                let _ = writeln!(out, "- `{}`", record.path);
            }
        }

        if !self.failures.is_empty() {
            let _ = writeln!(out, "\n## Not classified\n");
            for (path, error) in self.failures.iter().take(PER_DIMENSION) {
                let _ = writeln!(out, "- `{path}`: {error}");
            }
        }

        out
    }

    pub fn write(&self, dir: &Path) -> Result<(PathBuf, PathBuf)> {
        std::fs::create_dir_all(dir)?;
        let json_path = dir.join("architecture-pain.json");
        let md_path = dir.join("ARCHITECTURE-PAIN.md");
        std::fs::write(&json_path, serde_json::to_vec_pretty(&self.json())?)?;
        std::fs::write(&md_path, self.markdown())?;
        Ok((json_path, md_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jev::Scores;

    fn record(path: &str, worst: &str, values: [f32; 6]) -> Record {
        Record {
            path: path.into(),
            lines: 100,
            role: "data-access".into(),
            role_confidence: 0.9,
            scores: Scores {
                boundary_leak: values[0],
                complexity: values[1],
                data_access_cost: values[2],
                failure_handling: values[3],
                interaction_cost: values[4],
                trust_boundary_risk: values[5],
            },
            worst_dimension: worst.into(),
            worst_score: values.iter().cloned().fold(f32::MIN, f32::max),
            total_score: values.iter().sum(),
            needs_review: false,
            digest: "cafe".into(),
        }
    }

    fn report() -> Report {
        Report {
            records: vec![
                record("calm.ts", "complexity", [0.2, 0.9, 0.0, 0.1, 0.0, 0.0]),
                record("hot.ts", "data_access_cost", [1.0, 1.2, 2.9, 0.4, 0.0, 0.1]),
            ],
            failures: vec![("broken.ts".into(), "429 rate limited".into())],
            usage: Usage::default(),
            root: PathBuf::from("/repo"),
        }
    }

    #[test]
    fn the_painful_file_leads_the_report() {
        let markdown = report().markdown();
        let hot = markdown.find("hot.ts").expect("hot listed");
        let calm = markdown.find("calm.ts").expect("calm listed");
        assert!(hot < calm);
        assert!(markdown.contains("aws.amazon.com/builders-library"));
        assert!(markdown.contains("broken.ts"));
    }

    #[test]
    fn a_written_report_loads_back_as_a_cache() {
        let dir = std::env::temp_dir().join("painpoints-cache-test");
        let _ = std::fs::remove_dir_all(&dir);
        report().write(&dir).expect("written");
        let cache = load(&dir);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache["hot.ts"].worst_dimension, "data_access_cost");
        assert_eq!(cache["hot.ts"].digest, "cafe");
        assert!(load(Path::new("nowhere-at-all")).is_empty());
    }

    #[test]
    fn json_counts_only_real_pain_points() {
        let value = report().json();
        assert_eq!(value["summary"]["pain_points"], 1);
        assert_eq!(value["summary"]["by_dimension"]["data_access_cost"], 1);
        assert_eq!(value["summary"]["by_dimension"]["complexity"], 0);
        assert_eq!(value["files"][0]["path"], "hot.ts");
    }
}
