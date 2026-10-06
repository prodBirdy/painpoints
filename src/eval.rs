use crate::decisions::{self, DETECTORS};
use crate::scan::windows_of;
use crate::systemone::{model, provider_label, Client, Record, Usage, PAIN_THRESHOLD};
use anyhow::Result;
use futures::stream::{FuturesUnordered, StreamExt};

/// Labelled files compiled into the binary: each planted file holds exactly
/// one bad decision, each clean file does the same job correctly. Planted
/// and clean files are both negatives for every detector they are not
/// labelled with, so false alarms are measured on all of them.
pub struct Case {
    pub name: &'static str,
    pub language: &'static str,
    pub source: &'static str,
    pub planted: Option<&'static str>,
}

macro_rules! case {
    ($name:literal, $language:literal, $planted:expr) => {
        Case {
            name: $name,
            language: $language,
            source: include_str!(concat!("../evals/cases/", $name)),
            planted: $planted,
        }
    };
}

pub const CASES: [Case; 14] = [
    case!("order-totals.ts", "typescript", Some("query_in_loop")),
    case!("order-totals-clean.ts", "typescript", None),
    case!("report-export.ts", "typescript", Some("unbounded_read")),
    case!("settings-loader.ts", "typescript", Some("swallowed_error")),
    case!("settings-loader-clean.ts", "typescript", None),
    case!("payment-client.ts", "typescript", Some("unsafe_retry")),
    case!("user-search.py", "python", Some("injection")),
    case!("user-search-clean.py", "python", None),
    case!(
        "invoice-route.ts",
        "typescript",
        Some("missing_ownership_check")
    ),
    case!("invoice-route-clean.ts", "typescript", None),
    case!("email-config.ts", "typescript", Some("hardcoded_secret")),
    case!("upload-handler.ts", "typescript", Some("raw_error_to_user")),
    case!("notifier.ts", "typescript", Some("speculative_abstraction")),
    case!("ProfilePage.tsx", "typescript-react", Some("layer_mixing")),
];

fn probability(record: &Record, id: &str) -> f32 {
    record
        .decisions
        .iter()
        .find(|d| d.id == id)
        .map(|d| d.probability)
        .unwrap_or(0.0)
}

pub fn run() -> Result<()> {
    let client = Client::new()?;
    let runtime = tokio::runtime::Runtime::new()?;
    eprintln!(
        "evaluating {} labelled files with {} {}",
        CASES.len(),
        provider_label(),
        model()
    );

    let results: Vec<(usize, Record, Usage)> = runtime.block_on(async {
        let mut pending: FuturesUnordered<_> = CASES
            .iter()
            .enumerate()
            .map(|(i, case)| {
                let client = &client;
                async move {
                    let windows =
                        windows_of(&format!("src/{}", case.name), case.language, case.source);
                    client
                        .classify(&windows, None)
                        .await
                        .map(|(record, usage)| (i, record, usage))
                }
            })
            .collect();
        let mut out = Vec::new();
        while let Some(result) = pending.next().await {
            out.push(result?);
        }
        anyhow::Ok(out)
    })?;
    let mut records: Vec<Option<Record>> = vec![None; CASES.len()];
    let mut usage = Usage::default();
    for (i, record, used) in results {
        usage.add(used);
        records[i] = Some(record);
    }
    let records: Vec<Record> = records
        .into_iter()
        .map(|r| r.expect("every case"))
        .collect();

    println!(
        "{:<26} {:>8} {:>10}  worst miss in",
        "detector", "planted", "worst miss"
    );
    let (mut caught, mut alarms, mut negatives) = (0, 0, 0);
    for detector in &DETECTORS {
        let mut planted = None;
        let mut worst = (0.0f32, "-");
        for (case, record) in CASES.iter().zip(&records) {
            let p = probability(record, detector.id);
            if case.planted == Some(detector.id) {
                planted = Some(p);
                caught += usize::from(p >= decisions::ACT);
            } else {
                negatives += 1;
                alarms += usize::from(p >= decisions::ACT);
                if p > worst.0 {
                    worst = (p, case.name);
                }
            }
        }
        println!(
            "{:<26} {:>8} {:>10.2}  {}",
            detector.id,
            planted
                .map(|p| format!("{p:.2}"))
                .unwrap_or_else(|| "-".into()),
            worst.0,
            worst.1
        );
    }

    let planted_total = CASES.iter().filter(|c| c.planted.is_some()).count();
    let dimension_caught = CASES
        .iter()
        .zip(&records)
        .filter_map(|(case, record)| {
            let detector = decisions::detector(case.planted?)?;
            Some(record.scores.get(detector.dimension) >= PAIN_THRESHOLD)
        })
        .filter(|hit| *hit)
        .count();
    println!();
    println!(
        "detectors at {:.2}: caught {caught}/{planted_total} planted, {alarms} false alarms in {negatives} checks",
        decisions::ACT
    );
    println!(
        "dimension scores at {PAIN_THRESHOLD:.1}: caught {dimension_caught}/{planted_total} planted"
    );
    println!(
        "{} input tokens, {} output tokens",
        usage.input_tokens, usage.output_tokens
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_detector_has_a_planted_case_and_cases_are_real_files() {
        for detector in &DETECTORS {
            assert!(
                CASES.iter().any(|c| c.planted == Some(detector.id)),
                "no planted case for {}",
                detector.id
            );
        }
        for case in &CASES {
            assert!(!case.source.trim().is_empty(), "{} is empty", case.name);
        }
    }
}
