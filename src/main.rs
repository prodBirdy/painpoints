mod jev;
mod mcp;
mod report;
mod run;
mod scan;
mod ui;

use anyhow::{bail, Result};
use report::Report;
use scan::Config;
use std::path::PathBuf;

const USAGE: &str = "\
painpoints [TARGET] [options]
painpoints mcp

  TARGET            a repository to classify, or a single source file
                    (default: PAINPOINTS_ROOT or the current directory)
  --include DIR     only walk this subdirectory, repeatable
  --limit N         stop after N files
  --out DIR         where the report is written (default: ROOT/.painpoints)
  --headless        write the report without opening a window
  --json            print the result to stdout as JSON and write nothing else
  --refresh         reclassify everything instead of reusing the saved report
  -h, --help        this text

  mcp               serve the Model Context Protocol on stdio, exposing
                    painpoints_file and painpoints_repo

Needs TYPESAFE_API_KEY in the environment.";

#[derive(PartialEq)]
enum Mode {
    Window,
    Headless,
    Json,
}

struct Args {
    config: Config,
    out: Option<PathBuf>,
    mode: Mode,
    refresh: bool,
}

fn parse() -> Result<Option<Args>> {
    let mut target: Option<PathBuf> = None;
    let mut includes = Vec::new();
    let mut limit = 0;
    let mut out = None;
    let mut mode = Mode::Window;
    let mut refresh = false;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--headless" => mode = Mode::Headless,
            "--json" => mode = Mode::Json,
            "--refresh" => refresh = true,
            "--include" => match args.next() {
                Some(value) => includes.push(value),
                None => bail!("--include needs a directory"),
            },
            "--limit" => match args.next() {
                Some(value) => limit = value.parse()?,
                None => bail!("--limit needs a number"),
            },
            "--out" => match args.next() {
                Some(value) => out = Some(PathBuf::from(value)),
                None => bail!("--out needs a directory"),
            },
            other if other.starts_with('-') => bail!("unknown option {other}"),
            other if target.is_none() => target = Some(PathBuf::from(other)),
            other => bail!("unexpected argument {other}"),
        }
    }

    let target = scan::absolute(target.unwrap_or_else(scan::default_root));
    let mut config = if target.is_file() {
        Config::single(target)
    } else if target.is_dir() {
        Config::new(target)
    } else {
        bail!("{} does not exist", target.display());
    };
    config.includes = includes;
    config.limit = limit;

    Ok(Some(Args {
        config,
        out,
        mode,
        refresh,
    }))
}

fn collect(args: &Args, quiet: bool) -> Result<(Report, run::Outcome, PathBuf)> {
    let root = args.config.root.clone();
    let dir = args
        .out
        .clone()
        .unwrap_or_else(|| root.join(".painpoints"));
    let cache = if args.refresh {
        Default::default()
    } else {
        report::load(&dir)
    };

    let mut announced = false;
    let outcome = run::blocking(args.config.clone(), cache, |done, total| {
        if quiet {
            return;
        }
        if !announced {
            eprintln!("classifying {total} files");
            announced = true;
        }
        if done % 25 == 0 || done == total {
            eprintln!("  {done}/{total}");
        }
    })?;

    let report = Report {
        records: outcome.records.clone(),
        failures: outcome.failures.clone(),
        usage: outcome.usage,
        root,
    };
    Ok((report, outcome, dir))
}

fn headless(args: Args) -> Result<()> {
    let (report, outcome, dir) = collect(&args, false)?;
    let (json, markdown) = report.write(&dir)?;
    eprintln!(
        "{} files ({} reused), {} pain points, {} in / {} out tokens",
        report.records.len(),
        outcome.cached,
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

fn emit_json(args: Args) -> Result<()> {
    let single = args.config.only.is_some();
    let (report, outcome, dir) = collect(&args, true)?;

    let value = if single {
        let Some(record) = report.records.first() else {
            match outcome.failures.first() {
                Some((path, error)) => bail!("{path}: {error}"),
                None => bail!("not a source file painpoints recognises"),
            }
        };
        report::merge_write(&dir, &report.root, &report.records, report.usage)?;
        let mut value = mcp::file_result(record, outcome.cached == 1);
        value["tokens"] = serde_json::json!(report.usage);
        value
    } else {
        report.write(&dir)?;
        report.json()
    };

    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        return mcp::serve();
    }

    let Some(args) = parse()? else {
        println!("{USAGE}");
        return Ok(());
    };

    match args.mode {
        Mode::Headless => return headless(args),
        Mode::Json => return emit_json(args),
        Mode::Window => {}
    }

    let root = args.config.root.clone();
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| root.join(".painpoints"));
    let cache = if args.refresh {
        Default::default()
    } else {
        report::load(&out)
    };
    let mut rx = Some(run::stream(args.config, cache));
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
