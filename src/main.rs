#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod jev;
mod report;
mod scan;
mod ui;

use anyhow::{bail, Result};
use report::Report;
use scan::{Config, Event};
use std::path::PathBuf;

const USAGE: &str = "\
painpoints [ROOT] [options]

  ROOT              repository to classify (default: PAINPOINTS_ROOT or the current directory)
  --include DIR     only walk this subdirectory, repeatable (default: the whole repository)
  --limit N         stop after N files
  --out DIR         where the report is written (default: ROOT/.painpoints)
  --headless        write the report without opening a window
  --refresh         reclassify everything instead of reusing the saved report
  -h, --help        this text

Needs TYPESAFE_API_KEY in the environment.";

struct Args {
    config: Config,
    out: Option<PathBuf>,
    headless: bool,
    refresh: bool,
}

fn parse() -> Result<Option<Args>> {
    let mut config = Config::new(scan::default_root());
    let mut out = None;
    let mut headless = false;
    let mut refresh = false;
    let mut root_seen = false;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--headless" => headless = true,
            "--refresh" => refresh = true,
            "--include" => match args.next() {
                Some(value) => config.includes.push(value),
                None => bail!("--include needs a directory"),
            },
            "--limit" => match args.next() {
                Some(value) => config.limit = value.parse()?,
                None => bail!("--limit needs a number"),
            },
            "--out" => match args.next() {
                Some(value) => out = Some(PathBuf::from(value)),
                None => bail!("--out needs a directory"),
            },
            other if other.starts_with('-') => bail!("unknown option {other}"),
            other if !root_seen => {
                config.root = PathBuf::from(other);
                root_seen = true;
            }
            other => bail!("unexpected argument {other}"),
        }
    }

    if !config.root.is_dir() {
        bail!("{} is not a directory", config.root.display());
    }
    config.root = match config.root.canonicalize() {
        Ok(path) => match path.to_str().and_then(|p| p.strip_prefix("\\\\?\\")) {
            Some(stripped) => PathBuf::from(stripped),
            None => path,
        },
        Err(_) => config.root,
    };
    Ok(Some(Args {
        config,
        out,
        headless,
        refresh,
    }))
}

fn start(
    config: Config,
    cache: std::collections::HashMap<String, jev::Record>,
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
                usage: jev::Usage::default(),
                cached: 0,
            });
        }
    });
    rx
}

fn headless(args: Args) -> Result<()> {
    let root = args.config.root.clone();
    let out = args.out.clone().unwrap_or_else(|| root.join(".painpoints"));
    let cache = if args.refresh {
        Default::default()
    } else {
        report::load(&out)
    };
    let mut rx = start(args.config, cache);
    let mut report = Report {
        records: Vec::new(),
        failures: Vec::new(),
        usage: jev::Usage::default(),
        root,
    };
    let mut total = 0usize;
    let mut reused = 0usize;

    while let Some(event) = rx.blocking_recv() {
        match event {
            Event::Started { total: n } => {
                total = n;
                eprintln!("classifying {n} files");
            }
            Event::Done(record) => {
                report.records.push(*record);
                let done = report.records.len() + report.failures.len();
                if done % 25 == 0 || done == total {
                    eprintln!("  {done}/{total}");
                }
            }
            Event::Failed { path, error } => report.failures.push((path, error)),
            Event::Finished { usage, cached } => {
                report.usage = usage;
                reused = cached;
                break;
            }
        }
    }

    let (json, markdown) = report.write(&out)?;
    eprintln!(
        "{} files ({reused} reused), {} pain points, {} in / {} out tokens",
        report.records.len(),
        report
            .records
            .iter()
            .filter(|r| r.worst_score >= jev::PAIN_THRESHOLD)
            .count(),
        report.usage.input_tokens,
        report.usage.output_tokens
    );
    println!("{}", json.display());
    println!("{}", markdown.display());
    Ok(())
}

fn main() -> Result<()> {
    let Some(args) = parse()? else {
        println!("{USAGE}");
        return Ok(());
    };
    if args.headless {
        return headless(args);
    }

    let root = args.config.root.clone();
    let out = args.out.clone().unwrap_or_else(|| root.join(".painpoints"));
    let cache = if args.refresh {
        Default::default()
    } else {
        report::load(&out)
    };
    let mut rx = Some(start(args.config, cache));
    gpui::Application::new().run(move |cx: &mut gpui::App| {
        ui::open(
            rx.take().expect("run called once"),
            root.clone(),
            out.clone(),
            cx,
        );
    });
    Ok(())
}
