use crate::jev::{Record, Usage};
use crate::scan::{self, Config, Event};
use anyhow::Result;
use std::collections::HashMap;

pub struct Outcome {
    pub records: Vec<Record>,
    pub failures: Vec<(String, String)>,
    pub usage: Usage,
    pub cached: usize,
    pub total: usize,
}

pub fn stream(
    config: Config,
    cache: HashMap<String, Record>,
) -> tokio::sync::mpsc::UnboundedReceiver<Event> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("tokio runtime");
        if let Err(err) = rt.block_on(scan::run(config, cache, tx.clone())) {
            let _ = tx.send(Event::Failed {
                path: "<startup>".into(),
                error: err.to_string(),
            });
            let _ = tx.send(Event::Finished {
                usage: Usage::default(),
                cached: 0,
            });
        }
    });
    rx
}

pub fn blocking(
    config: Config,
    cache: HashMap<String, Record>,
    mut progress: impl FnMut(usize, usize),
) -> Result<Outcome> {
    let mut rx = stream(config, cache);
    let mut outcome = Outcome {
        records: Vec::new(),
        failures: Vec::new(),
        usage: Usage::default(),
        cached: 0,
        total: 0,
    };

    while let Some(event) = rx.blocking_recv() {
        match event {
            Event::Started { total } => outcome.total = total,
            Event::Done(record) => {
                outcome.records.push(*record);
                progress(
                    outcome.records.len() + outcome.failures.len(),
                    outcome.total,
                );
            }
            Event::Failed { path, error } => outcome.failures.push((path, error)),
            Event::Finished { usage, cached } => {
                outcome.usage = usage;
                outcome.cached = cached;
                break;
            }
        }
    }

    outcome.records.sort_by(|a, b| a.rank_key().cmp(&b.rank_key()));
    Ok(outcome)
}
