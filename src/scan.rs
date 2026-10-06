use crate::rules::{self, RulesFile};
use crate::systemone::{digest, Client, FileState, Record, Usage};
use anyhow::Result;
use futures::stream::{FuturesUnordered, StreamExt};
use ignore::WalkBuilder;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const MAX_CHARS: usize = 8000;
pub const MAX_WINDOWS: usize = 6;
const OVERLAP_LINES: usize = 20;
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
        let walker = walk_builder(&dir, &config.root).build();
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

/// Linked worktrees store a gitfile at `.git` (`gitdir: …`) instead of a
/// directory. `ignore::WalkBuilder`'s default `require_git` + `git_exclude` +
/// parent-ignore walk then treats the checkout as ignored and yields zero
/// files. Honour this tree's `.gitignore` without requiring a `.git`
/// directory, skip `info/exclude` (it lives in the main worktree), and do not
/// inherit parent-repo ignore rules that often list the worktree path itself.
fn walk_builder(dir: &Path, repo_root: &Path) -> WalkBuilder {
    let mut builder = WalkBuilder::new(dir);
    builder.filter_entry(|entry| {
        entry
            .file_name()
            .to_str()
            .is_none_or(|name| !SKIP_DIRS.contains(&name))
    });
    if repo_root.join(".git").is_file() || dir.join(".git").is_file() {
        builder.require_git(false);
        builder.git_exclude(false);
        builder.parents(false);
    }
    builder
}

/// Reads a file as the windows the model will see: consecutive slices of
/// whole lines up to `MAX_CHARS` each, overlapping by a few lines so a
/// construct cut at a boundary is still seen whole once, and at most
/// `MAX_WINDOWS` of them.
pub fn read_windows(root: &Path, file: &Path) -> Result<Vec<FileState>> {
    let raw = std::fs::read_to_string(file)?;
    let rel = file
        .strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/");
    Ok(windows_of(&rel, language(file).unwrap_or("unknown"), &raw))
}

pub fn windows_of(rel: &str, language: &str, raw: &str) -> Vec<FileState> {
    let total = raw.lines().count();
    let lines: Vec<&str> = raw.split_inclusive('\n').collect();

    let mut windows: Vec<FileState> = Vec::new();
    let mut start = 0usize;
    while start < lines.len() && windows.len() < MAX_WINDOWS {
        let mut end = start;
        let mut size = 0usize;
        while end < lines.len() && (end == start || size + lines[end].len() <= MAX_CHARS) {
            size += lines[end].len();
            end += 1;
        }
        let mut source = lines[start..end].concat();
        if source.len() > MAX_CHARS {
            let mut cut = MAX_CHARS;
            while !source.is_char_boundary(cut) {
                cut -= 1;
            }
            source.truncate(cut);
        }
        windows.push(FileState {
            path: rel.to_string(),
            language: language.to_string(),
            lines: total,
            start_line: start + 1,
            end_line: end,
            truncated: false,
            source,
        });
        if end >= lines.len() {
            break;
        }
        let overlap = OVERLAP_LINES.min((end - start) / 4);
        start = end - overlap;
    }
    if let Some(last) = windows.last_mut() {
        last.truncated = last.end_line < lines.len();
    }
    windows
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
    Classify(Vec<FileState>),
    Failed(String, String),
}

pub async fn run(
    config: Config,
    cache: HashMap<String, Record>,
    tx: tokio::sync::mpsc::UnboundedSender<Event>,
) -> Result<()> {
    let files = collect_files(&config);
    let _ = tx.send(Event::Started { total: files.len() });
    let agent_rules: Option<RulesFile> = match rules::load_or_compile(&config.root) {
        Ok(value) => value,
        Err(err) => {
            let _ = tx.send(Event::Failed {
                path: "<rules>".into(),
                error: err.to_string(),
            });
            None
        }
    };

    let mut cached = 0usize;
    let mut queue = Vec::new();
    for file in files {
        let job = match read_windows(&config.root, &file) {
            Ok(windows) if windows.is_empty() => continue,
            Ok(windows) => match cache
                .get(&windows[0].path)
                .filter(|record| record.digest == digest(&windows, agent_rules.as_ref()))
            {
                Some(record) => Job::Ready(Box::new(record.clone())),
                None => Job::Classify(windows),
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
            Job::Classify(windows) => queue.push(windows),
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
    let agent_rules = Arc::new(agent_rules);
    let mut total = Usage::default();
    let mut pending = FuturesUnordered::new();
    let mut queue = queue.into_iter();

    let spawn_next = |queue: &mut std::vec::IntoIter<Vec<FileState>>,
                      pending: &mut FuturesUnordered<_>| {
        if let Some(windows) = queue.next() {
            let client = client.clone();
            let agent_rules = agent_rules.clone();
            pending.push(async move {
                client
                    .classify(&windows, agent_rules.as_ref().as_ref())
                    .await
                    .map_err(|err| (windows[0].path.clone(), err.to_string()))
            });
        }
    };

    for _ in 0..CONCURRENCY {
        spawn_next(&mut queue, &mut pending);
    }

    while let Some(result) = pending.next().await {
        match result {
            Ok((record, usage)) => {
                total.add(usage);
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
        assert!(files.iter().any(|f| f.ends_with("systemone.rs")));
        assert!(!files.iter().any(|f| f.to_string_lossy().contains("target")));
    }

    #[test]
    fn a_short_file_is_one_window_with_every_line() {
        let windows = windows_of("src/a.ts", "typescript", "const a = 1;\nconst b = 2;\n");
        assert_eq!(windows.len(), 1);
        assert_eq!((windows[0].start_line, windows[0].end_line), (1, 2));
        assert!(!windows[0].truncated);
        assert_eq!(windows[0].source, "const a = 1;\nconst b = 2;\n");
    }

    #[test]
    fn a_long_file_is_read_whole_in_overlapping_windows() {
        let line = format!("{}\n", "x".repeat(99));
        let raw = line.repeat(200);
        let windows = windows_of("src/big.ts", "typescript", &raw);
        assert!(windows.len() > 1);
        assert_eq!(windows[0].start_line, 1);
        assert_eq!(windows.last().unwrap().end_line, 200);
        assert!(!windows.last().unwrap().truncated);
        for pair in windows.windows(2) {
            assert!(
                pair[1].start_line <= pair[0].end_line,
                "windows must overlap"
            );
            assert!(pair[1].end_line > pair[0].end_line, "windows must advance");
        }
        assert!(windows
            .iter()
            .all(|w| w.source.len() <= MAX_CHARS && w.lines == 200));
    }

    #[test]
    fn a_file_past_the_window_budget_marks_its_last_window_truncated() {
        let line = format!("{}\n", "y".repeat(99));
        let raw = line.repeat(80 * (MAX_WINDOWS + 2));
        let windows = windows_of("src/huge.ts", "typescript", &raw);
        assert_eq!(windows.len(), MAX_WINDOWS);
        assert!(windows.last().unwrap().truncated);
        assert!(windows[..MAX_WINDOWS - 1].iter().all(|w| !w.truncated));
    }

    #[test]
    fn a_single_line_longer_than_a_window_is_cut_on_a_char_boundary() {
        let raw = "é".repeat(MAX_CHARS);
        let windows = windows_of("src/min.js", "javascript", &raw);
        assert_eq!(windows.len(), 1);
        assert!(windows[0].source.len() <= MAX_CHARS);
    }

    fn scan_fixture(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("painpoints-scan-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        dir
    }

    #[test]
    fn a_gitfile_at_the_root_does_not_hide_source_files() {
        let dir = scan_fixture("gitfile");
        std::fs::write(dir.join(".git"), "gitdir: /tmp/fake.git/worktrees/wt\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "skip_me.rs\n").unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn ok() {}\n").unwrap();
        std::fs::write(dir.join("src/skip_me.rs"), "pub fn no() {}\n").unwrap();

        let files = collect_files(&Config::new(dir.clone()));
        assert!(
            files.iter().any(|f| f.ends_with("lib.rs")),
            "worktree with a .git file should still walk source files, got {files:?}"
        );
        assert!(
            !files.iter().any(|f| f.ends_with("skip_me.rs")),
            "local .gitignore should still apply when .git is a file, got {files:?}"
        );

        let mut included = Config::new(dir);
        included.includes = vec!["src".into()];
        let files = collect_files(&included);
        assert!(
            files.iter().any(|f| f.ends_with("lib.rs")),
            "--include of a worktree subdir must see files, got {files:?}"
        );
    }
}
