use crate::jev::{digest, Client, FileState, Record, Usage};
use crate::rubric::{self, Rubric};
use anyhow::Result;
use futures::stream::{FuturesUnordered, StreamExt};
use ignore::WalkBuilder;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const MAX_CHARS: usize = 8000;
pub const CONCURRENCY: usize = 12;
const MAX_FILE_BYTES: u64 = 400_000;

const LANGUAGES: [(&str, &str); 26] = [
    ("ts", "typescript"),
    ("tsx", "typescript-react"),
    ("mts", "typescript"),
    ("cts", "typescript"),
    ("js", "javascript"),
    ("jsx", "javascript-react"),
    ("mjs", "javascript"),
    ("cjs", "javascript"),
    ("vue", "vue"),
    ("svelte", "svelte"),
    ("astro", "astro"),
    ("py", "python"),
    ("rb", "ruby"),
    ("go", "go"),
    ("rs", "rust"),
    ("java", "java"),
    ("kt", "kotlin"),
    ("kts", "kotlin"),
    ("cs", "csharp"),
    ("php", "php"),
    ("swift", "swift"),
    ("scala", "scala"),
    ("ex", "elixir"),
    ("exs", "elixir"),
    ("dart", "dart"),
    ("sql", "sql"),
];

const SKIP_DIRS: [&str; 13] = [
    "node_modules",
    "dist",
    "build",
    "out",
    "target",
    "vendor",
    "coverage",
    "__pycache__",
    "__snapshots__",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".venv",
];

const SKIP_SUFFIXES: [&str; 5] = [".d.ts", ".min.js", ".min.mjs", ".bundle.js", "_pb.js"];

#[derive(Debug, Clone)]
pub struct Config {
    pub root: PathBuf,
    pub includes: Vec<String>,
    pub only: Option<PathBuf>,
    pub limit: usize,
}

impl Config {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            includes: Vec::new(),
            only: None,
            limit: 0,
        }
    }

    pub fn single(file: PathBuf) -> Self {
        let mut config = Self::new(repo_root_of(&file));
        config.only = Some(file);
        config
    }
}

pub fn absolute(path: PathBuf) -> PathBuf {
    match path.canonicalize() {
        Ok(full) => match full.to_str().and_then(|p| p.strip_prefix(r"\\?\")) {
            Some(stripped) => PathBuf::from(stripped),
            None => full,
        },
        Err(_) => path,
    }
}

pub fn repo_root_of(file: &Path) -> PathBuf {
    let mut current = file.parent();
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return dir.to_path_buf();
        }
        current = dir.parent();
    }
    file.parent().unwrap_or(Path::new(".")).to_path_buf()
}

pub fn default_root() -> PathBuf {
    std::env::var("PAINPOINTS_ROOT")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn language(path: &Path) -> Option<&'static str> {
    let ext = path.extension().and_then(|e| e.to_str())?;
    LANGUAGES
        .iter()
        .find(|(candidate, _)| *candidate == ext)
        .map(|(_, name)| *name)
}

pub fn collect_files(config: &Config) -> Vec<PathBuf> {
    if let Some(file) = &config.only {
        return match file.is_file() && language(file).is_some() {
            true => vec![file.clone()],
            false => Vec::new(),
        };
    }
    let dirs: Vec<PathBuf> = if config.includes.is_empty() {
        vec![config.root.clone()]
    } else {
        config
            .includes
            .iter()
            .map(|sub| config.root.join(sub))
            .collect()
    };

    let mut out = Vec::new();
    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        let walker = WalkBuilder::new(&dir)
            .filter_entry(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_none_or(|name| !SKIP_DIRS.contains(&name))
            })
            .build();
        for entry in walker.flatten() {
            let path = entry.path();
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if SKIP_SUFFIXES.iter().any(|suffix| name.ends_with(suffix)) {
                continue;
            }
            if language(path).is_none() {
                continue;
            }
            if entry
                .metadata()
                .is_ok_and(|meta| meta.len() == 0 || meta.len() > MAX_FILE_BYTES)
            {
                continue;
            }
            out.push(path.to_path_buf());
        }
    }
    out.sort();
    out.dedup();
    if config.limit > 0 {
        out.truncate(config.limit);
    }
    out
}

pub fn read_state(root: &Path, file: &Path) -> Result<FileState> {
    let raw = std::fs::read_to_string(file)?;
    let rel = file
        .strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/");
    let mut end = MAX_CHARS.min(raw.len());
    while end > 0 && !raw.is_char_boundary(end) {
        end -= 1;
    }
    Ok(FileState {
        lines: raw.lines().count(),
        truncated: raw.len() > end,
        source: raw[..end].to_string(),
        language: language(file).unwrap_or("unknown").to_string(),
        path: rel,
    })
}

#[derive(Debug, Clone)]
pub enum Event {
    Started { total: usize },
    Done(Box<Record>),
    Failed { path: String, error: String },
    Finished { usage: Usage, cached: usize },
}

enum Job {
    Ready(Box<Record>),
    Classify(Box<FileState>),
    Failed(String, String),
}

pub async fn run(
    config: Config,
    cache: HashMap<String, Record>,
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> Result<()> {
    let files = collect_files(&config);
    let _ = tx.send(Event::Started { total: files.len() });
    let rubric: Option<Rubric> = match rubric::load_or_compile(&config.root) {
        Ok(value) => value,
        Err(err) => {
            let _ = tx.send(Event::Failed {
                path: "<rubric>".into(),
                error: err.to_string(),
            });
            None
        }
    };

    let mut cached = 0usize;
    let mut queue = Vec::new();
    for file in files {
        let job = match read_state(&config.root, &file) {
            Ok(state) => match cache
                .get(&state.path)
                .filter(|record| record.digest == digest(&state, rubric.as_ref()))
            {
                Some(record) => Job::Ready(Box::new(record.clone())),
                None => Job::Classify(Box::new(state)),
            },
            Err(err) => Job::Failed(file.to_string_lossy().to_string(), err.to_string()),
        };
        match job {
            Job::Ready(record) => {
                cached += 1;
                let _ = tx.send(Event::Done(record));
            }
            Job::Failed(path, error) => {
                let _ = tx.send(Event::Failed { path, error });
            }
            Job::Classify(state) => queue.push(*state),
        }
    }

    if queue.is_empty() {
        let _ = tx.send(Event::Finished {
            usage: Usage::default(),
            cached,
        });
        return Ok(());
    }

    let client = Arc::new(Client::new()?);
    let rubric = Arc::new(rubric);
    let mut total = Usage::default();
    let mut pending = FuturesUnordered::new();
    let mut queue = queue.into_iter();

    let spawn_next = |queue: &mut std::vec::IntoIter<FileState>,
                      pending: &mut FuturesUnordered<_>| {
        if let Some(state) = queue.next() {
            let client = client.clone();
            let rubric = rubric.clone();
            pending.push(async move {
                client
                    .classify(&state, rubric.as_ref().as_ref())
                    .await
                    .map_err(|err| (state.path.clone(), err.to_string()))
            });
        }
    };

    for _ in 0..CONCURRENCY {
        spawn_next(&mut queue, &mut pending);
    }

    while let Some(result) = pending.next().await {
        match result {
            Ok((record, usage)) => {
                total.input_tokens += usage.input_tokens;
                total.output_tokens += usage.output_tokens;
                let _ = tx.send(Event::Done(Box::new(record)));
            }
            Err((path, error)) => {
                let _ = tx.send(Event::Failed { path, error });
            }
        }
        spawn_next(&mut queue, &mut pending);
    }

    let _ = tx.send(Event::Finished {
        usage: total,
        cached,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_covers_the_source_files_and_skips_the_rest() {
        assert_eq!(language(Path::new("a/b.tsx")), Some("typescript-react"));
        assert_eq!(language(Path::new("a/b.py")), Some("python"));
        assert_eq!(language(Path::new("a/b.lock")), None);
        assert_eq!(language(Path::new("README")), None);
    }

    #[test]
    fn generated_and_vendored_paths_are_excluded() {
        assert!(SKIP_SUFFIXES.iter().any(|s| "types.d.ts".ends_with(s)));
        assert!(SKIP_DIRS.contains(&"node_modules"));
    }

    #[test]
    fn the_crate_itself_is_discoverable() {
        let mut config = Config::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        config.includes = vec!["src".into()];
        let files = collect_files(&config);
        assert!(files.iter().any(|f| f.ends_with("jev.rs")));
        assert!(!files.iter().any(|f| f.to_string_lossy().contains("target")));
    }
}
