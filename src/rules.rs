use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const RULES_VERSION: u32 = 1;
pub const RULES_FILE: &str = "rules.json";
pub const MAX_RULES: usize = 40;
pub const MAX_RULE_TEXT_CHARS: usize = 6000;
pub const MAX_RULE_QUESTIONS: usize = 20;
pub const DEFAULT_ACT: f32 = 0.8;
pub const DEFAULT_FLAG: f32 = 0.5;

const ROOT_NAMES: &[&str] = &[
    "AGENTS.md",
    "AGENT.md",
    "CLAUDE.md",
    "AGENTS.txt",
    ".cursorrules",
];
const NESTED_NAMES: &[&str] = &["AGENTS.md", "CLAUDE.md"];
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "dist",
    "build",
    "out",
    "target",
    "vendor",
    "coverage",
    "__pycache__",
    ".git",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".venv",
    ".turbo",
    ".cache",
    ".painpoints",
    ".abide",
];
const MAX_WALK_DEPTH: usize = 6;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RulesFile {
    pub version: u32,
    #[serde(rename = "compiledAt")]
    pub compiled_at: String,
    #[serde(rename = "compiledBy", skip_serializing_if = "Option::is_none")]
    pub compiled_by: Option<String>,
    pub sources: Vec<RulesSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thresholds: Option<Thresholds>,
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RulesSource {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Thresholds {
    pub act: f32,
    pub flag: f32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            act: DEFAULT_ACT,
            flag: DEFAULT_FLAG,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rule {
    pub id: String,
    pub text: String,
    pub source: RuleSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    pub check: Check,
    #[serde(
        default = "default_active",
        skip_serializing_if = "is_active"
    )]
    pub status: String,
}

fn default_active() -> String {
    "active".into()
}

fn is_active(status: &String) -> bool {
    status == "active"
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuleSource {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Check {
    #[serde(rename = "lint")]
    Lint {
        #[serde(skip_serializing_if = "Option::is_none")]
        how: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        overlaps: Option<String>,
    },
    #[serde(rename = "model")]
    Model {
        question: Question,
        #[serde(skip_serializing_if = "Option::is_none")]
        overlaps: Option<String>,
    },
    #[serde(rename = "deferred")]
    Deferred { reason: String },
    #[serde(rename = "unenforceable")]
    Unenforceable { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Question {
    #[serde(rename = "boolean")]
    Boolean {
        instructions: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        criteria: Option<BTreeMap<String, String>>,
    },
    #[serde(rename = "choice")]
    Choice {
        instructions: String,
        criteria: BTreeMap<String, String>,
        violating: Vec<String>,
    },
    #[serde(rename = "score")]
    Score {
        instructions: String,
        criteria: Vec<String>,
        #[serde(rename = "violatingFrom")]
        violating_from: usize,
    },
}

impl Question {
    pub fn to_jev(&self) -> Value {
        match self {
            Question::Boolean {
                instructions,
                criteria,
            } => {
                let mut value = json!({ "type": "boolean", "instructions": instructions });
                if let Some(criteria) = criteria {
                    value["criteria"] = json!(criteria);
                }
                value
            }
            Question::Choice {
                instructions,
                criteria,
                violating: _,
            } => json!({
                "type": "choice",
                "instructions": instructions,
                "criteria": criteria,
            }),
            Question::Score {
                instructions,
                criteria,
                violating_from: _,
            } => json!({
                "type": "score",
                "instructions": instructions,
                "criteria": criteria,
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SourceCandidate {
    pub path: String,
    pub absolute: PathBuf,
    pub scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuleVerdict {
    #[serde(rename = "ruleId")]
    pub rule_id: String,
    pub text: String,
    pub source: String,
    pub probability: f32,
    pub band: String,
    #[serde(serialize_with = "round2")]
    pub score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

fn round2<S: serde::Serializer>(value: &f32, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_f64(((*value as f64) * 100.0).round() / 100.0)
}

impl RulesFile {
    pub fn thresholds(&self) -> Thresholds {
        self.thresholds.unwrap_or_default()
    }

    pub fn model_rules_for(&self, file: &str) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|rule| {
                rule.status == "active"
                    && matches!(rule.check, Check::Model { .. })
                    && rule.when.as_deref() != Some("turn")
                    && rule_applies_to(rule, file)
            })
            .take(MAX_RULE_QUESTIONS)
            .collect()
    }

    pub fn applied_digest(&self, file: &str) -> String {
        let rules = self.model_rules_for(file);
        if rules.is_empty() {
            return String::new();
        }
        let mut buf = Vec::new();
        for rule in rules {
            buf.extend_from_slice(rule.id.as_bytes());
            buf.push(0);
            buf.extend_from_slice(rule.text.as_bytes());
            buf.push(0);
            if let Check::Model { question, .. } = &rule.check {
                buf.extend_from_slice(serde_json::to_string(question).unwrap_or_default().as_bytes());
            }
            buf.push(b'\n');
        }
        sha256_hex(&buf)
    }

    pub fn bucket_counts(&self) -> (usize, usize, usize, usize) {
        let mut model = 0;
        let mut lint = 0;
        let mut deferred = 0;
        let mut unenforceable = 0;
        for rule in &self.rules {
            match rule.check {
                Check::Model { .. } => model += 1,
                Check::Lint { .. } => lint += 1,
                Check::Deferred { .. } => deferred += 1,
                Check::Unenforceable { .. } => unenforceable += 1,
            }
        }
        (model, lint, deferred, unenforceable)
    }
}

pub fn rules_path(root: &Path) -> PathBuf {
    root.join(".painpoints").join(RULES_FILE)
}

pub fn load(path: &Path) -> Option<RulesFile> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn write(path: &Path, rules: &RulesFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_vec_pretty(rules)?;
    std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))
}

pub fn is_stale(rules: &RulesFile, candidates: &[SourceCandidate], root: &Path) -> bool {
    let listed: HashSet<String> = rules
        .sources
        .iter()
        .map(|s| normalize_rel(&s.path))
        .collect();
    for source in &rules.sources {
        let absolute = resolve_source(root, &source.path);
        match std::fs::read(&absolute) {
            Ok(bytes) => match &source.sha {
                Some(sha) if sha == &sha256_hex(&bytes) => {}
                _ => return true,
            },
            Err(_) => return true,
        }
    }
    candidates
        .iter()
        .any(|c| !listed.contains(&normalize_rel(&c.path)))
}

pub fn load_or_compile(root: &Path) -> Result<Option<RulesFile>> {
    let candidates = discover(root);
    if candidates.is_empty() {
        return Ok(None);
    }
    let path = rules_path(root);
    if let Some(existing) = load(&path) {
        if !is_stale(&existing, &candidates, root) {
            return Ok(Some(existing));
        }
    }
    let compiled = compile(root, &candidates);
    write(&path, &compiled)?;
    Ok(Some(compiled))
}

pub fn discover(root: &Path) -> Vec<SourceCandidate> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();

    for name in ROOT_NAMES {
        push_file(root, root.join(name), "**/*", &mut found, &mut seen);
    }

    let copilot = root.join(".github/copilot-instructions.md");
    push_file(root, copilot, "**/*", &mut found, &mut seen);

    let claude_local = root.join(".claude/CLAUDE.md");
    push_file(root, claude_local, "**/*", &mut found, &mut seen);

    walk_nested(root, root, 1, &mut found, &mut seen);
    walk_cursor_rules(root, &mut found, &mut seen);

    let mut i = 0;
    while i < found.len() {
        if let Ok(text) = std::fs::read_to_string(&found[i].absolute) {
            if let Some(target) = pointer_target(&text) {
                let absolute = resolve_source(root, &target);
                let scope = found[i].scope.clone();
                push_file(root, absolute, &scope, &mut found, &mut seen);
            }
        }
        i += 1;
    }

    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

fn push_file(
    root: &Path,
    absolute: PathBuf,
    scope: &str,
    found: &mut Vec<SourceCandidate>,
    seen: &mut HashSet<String>,
) {
    if !absolute.is_file() {
        return;
    }
    let rel = rel_path(root, &absolute);
    if !seen.insert(normalize_rel(&rel)) {
        return;
    }
    found.push(SourceCandidate {
        path: rel,
        absolute,
        scope: scope.to_string(),
    });
}

fn walk_nested(
    root: &Path,
    dir: &Path,
    depth: usize,
    found: &mut Vec<SourceCandidate>,
    seen: &mut HashSet<String>,
) {
    if depth > MAX_WALK_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        if name.starts_with('.') && name != ".cursor" && name != ".claude" && name != ".github" {
            continue;
        }
        let rel = rel_path(root, &path);
        if !rel.is_empty() {
            for file_name in NESTED_NAMES {
                let file = path.join(file_name);
                let scope = format!("{rel}/**/*");
                push_file(root, file, &scope, found, seen);
            }
        }
        walk_nested(root, &path, depth + 1, found, seen);
    }
}

fn walk_cursor_rules(
    root: &Path,
    found: &mut Vec<SourceCandidate>,
    seen: &mut HashSet<String>,
) {
    let dir = root.join(".cursor/rules");
    if !dir.is_dir() {
        return;
    }
    let mut stack = vec![dir];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default();
            if ext != "md" && ext != "mdc" {
                continue;
            }
            let scope = cursor_rule_scope(root, &path);
            push_file(root, path, &scope, found, seen);
        }
    }
}

fn cursor_rule_scope(root: &Path, file: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(file) else {
        return "**/*".into();
    };
    if let Some(front) = frontmatter(&text) {
        if front_bool(&front, "alwaysApply").unwrap_or(false) {
            return "**/*".into();
        }
        if let Some(globs) = front_globs(&front) {
            return globs.join(",");
        }
    }
    let _ = root;
    "**/*".into()
}

pub fn compile(_root: &Path, candidates: &[SourceCandidate]) -> RulesFile {
    let mut sources = Vec::new();
    let mut rules = Vec::new();
    let mut used_ids = HashSet::new();
    let mut text_chars = 0usize;

    for candidate in candidates {
        let Ok(bytes) = std::fs::read(&candidate.absolute) else {
            continue;
        };
        let sha = sha256_hex(&bytes);
        let text = String::from_utf8_lossy(&bytes);
        if pointer_target(&text).is_some() {
            sources.push(RulesSource {
                path: candidate.path.clone(),
                sha: Some(sha),
                scope: Some(candidate.scope.clone()),
            });
            continue;
        }
        sources.push(RulesSource {
            path: candidate.path.clone(),
            sha: Some(sha),
            scope: Some(candidate.scope.clone()),
        });

        let scope_globs = scope_globs(&candidate.scope);
        for extracted in extract_statements(&candidate.path, &text) {
            if rules.len() >= MAX_RULES || text_chars + extracted.text.len() > MAX_RULE_TEXT_CHARS {
                break;
            }
            if rules.iter().any(|r: &Rule| same_rule(&r.text, &extracted.text)) {
                continue;
            }
            let id = unique_id(&extracted.text, &mut used_ids);
            let check = classify_rule(&extracted.text);
            let when = match check {
                Check::Model { .. } => Some("edit".into()),
                _ => None,
            };
            text_chars += extracted.text.len();
            rules.push(Rule {
                id,
                text: extracted.text,
                source: RuleSource {
                    path: candidate.path.clone(),
                    line: Some(extracted.line),
                },
                scope: scope_globs.clone(),
                when,
                check,
                status: "active".into(),
            });
        }
    }

    RulesFile {
        version: RULES_VERSION,
        compiled_at: utc_now(),
        compiled_by: Some("painpoints".into()),
        sources,
        thresholds: None,
        rules,
    }
}

struct Extracted {
    line: u32,
    text: String,
}

fn extract_statements(path: &str, text: &str) -> Vec<Extracted> {
    let body = strip_frontmatter(text);
    let prefix_len = text.len().saturating_sub(body.len());
    let leading_lines = if prefix_len == 0 {
        0
    } else {
        text[..prefix_len].lines().count() as u32
    };
    let _path = path;

    let mut out = Vec::new();
    let mut in_fence = false;
    for (i, raw) in body.lines().enumerate() {
        if raw.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let line = (i as u32) + 1 + leading_lines;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let item = match list_item(trimmed) {
            Some(item) => item,
            None if looks_like_instruction(trimmed) && trimmed.len() < 400 => {
                trimmed.to_string()
            }
            None => continue,
        };
        let item = clean_rule_text(&item);
        if item.is_empty() || !looks_like_instruction(&item) {
            continue;
        }
        if item.len() > 600 {
            continue;
        }
        out.push(Extracted { line, text: item });
    }
    out
}

fn list_item(line: &str) -> Option<String> {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
        return Some(rest.trim().to_string());
    }
    if let Some(rest) = line.strip_prefix("+ ") {
        return Some(rest.trim().to_string());
    }
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && i < bytes.len() && (bytes[i] == b'.' || bytes[i] == b')') {
        let rest = line[i + 1..].trim();
        if !rest.is_empty() {
            return Some(rest.to_string());
        }
    }
    None
}

fn clean_rule_text(text: &str) -> String {
    let mut text = text.trim().to_string();
    if (text.starts_with("**") && text.ends_with("**") && text.len() > 4)
        || (text.starts_with('*') && text.ends_with('*') && text.len() > 2 && !text.starts_with("**"))
    {
        text = text.trim_matches('*').trim().to_string();
    }
    if let Some((head, tail)) = text.split_once(":**") {
        let head = head.trim_start_matches('*').trim();
        let tail = tail.trim().trim_start_matches('*').trim();
        if !tail.is_empty() {
            text = format!("{head}: {tail}");
        }
    } else if let Some((head, tail)) = text.split_once(":** ") {
        text = format!("{}: {}", head.trim_matches('*').trim(), tail.trim());
    }
    text.trim_end_matches(|c| c == '.' || c == ';').trim().to_string()
}

fn looks_like_instruction(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.len() < 6 {
        return false;
    }
    if fluff(&lower) {
        return false;
    }
    const MARKERS: &[&str] = &[
        "don't",
        "do not",
        "never",
        "always",
        "must ",
        "must not",
        "should ",
        "avoid ",
        "prefer ",
        "use ",
        "do ",
        "keep ",
        "write ",
        "don't ",
        "no ",
        "not ",
        "only ",
        "require",
        "forbid",
        "disallow",
        "without ",
        "instead",
        "rather than",
    ];
    if MARKERS.iter().any(|m| lower.contains(m)) {
        return true;
    }
    const STARTS: &[&str] = &[
        "use ", "don't", "do not", "never ", "always ", "avoid ", "prefer ", "keep ",
        "write ", "make ", "put ", "leave ", "treat ", "score ", "run ", "ask ",
        "state ", "touch ", "match ", "remove ", "preserve ", "pass ", "verify ",
    ];
    STARTS.iter().any(|s| lower.starts_with(s))
}

fn fluff(lower: &str) -> bool {
    const SKIP: &[&str] = &[
        "table of contents",
        "overview",
        "introduction",
        "see also",
        "license",
        "related",
        "hard rules for anyone",
        "read fully before",
        "merge with project-specific",
        "these guidelines bias",
        "ask yourself",
        "the test:",
        "transform tasks",
        "for multi-step",
        "strong success",
        "weak criteria",
        "quality software is built",
        "pnpm monorepo",
    ];
    SKIP.iter().any(|s| lower.starts_with(s) || lower == *s)
}

fn classify_rule(text: &str) -> Check {
    let lower = text.to_ascii_lowercase();

    if conversation_rule(&lower) {
        return Check::Unenforceable {
            reason: "about the conversation, not the code".into(),
        };
    }
    if needs_repo_context(&lower) {
        return Check::Deferred {
            reason: "needs the rest of the repository".into(),
        };
    }
    if needs_counting(&lower) {
        return Check::Deferred {
            reason: "needs a script, not a judge".into(),
        };
    }
    if let Some((how, pattern)) = lint_shape(&lower, text) {
        return Check::Lint {
            how: Some(how),
            pattern: Some(pattern),
            overlaps: None,
        };
    }

    Check::Model {
        question: scaffold_question(text),
        overlaps: None,
    }
}

fn conversation_rule(lower: &str) -> bool {
    const MARKERS: &[&str] = &[
        "if uncertain, ask",
        "if unsure, ask",
        "ask when",
        "ask the user",
        "state your assumptions",
        "state a plan",
        "state a brief plan",
        "think before",
        "don't assume",
        "don't hide confusion",
        "surface tradeoffs",
        "run the tests before",
        "verify before claiming",
        "present them",
        "push back",
        "name what's confusing",
        "clean up processes",
        "do not mention",
        "don't mention this",
        "humanizer",
        "no ai attribution",
        "co-authored-by",
        "generated with",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

fn needs_repo_context(lower: &str) -> bool {
    const MARKERS: &[&str] = &[
        "reuse existing",
        "follow existing",
        "existing pattern",
        "already exist",
        "already exists",
        "check whether a helper",
        "looks like the rest",
        "match existing style",
        "in two places",
        "shared component",
        "don't invent near-duplicates",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

fn needs_counting(lower: &str) -> bool {
    const MARKERS: &[&str] = &[
        "line length",
        "lines long",
        "under 200 lines",
        "max lines",
        "maximum lines",
        "file size",
        "nesting depth",
        "alphabetical",
        "ordered by",
        "by line length",
        "no more than",
        "fewer than",
        "at most ",
        "character limit",
    ];
    if MARKERS.iter().any(|m| lower.contains(m)) {
        return true;
    }
    let has_number = lower.chars().any(|c| c.is_ascii_digit());
    has_number
        && (lower.contains(" lines") || lower.contains(" line ") || lower.contains("chars"))
        && (lower.contains("under")
            || lower.contains("over")
            || lower.contains("more than")
            || lower.contains("less than")
            || lower.contains("longer than")
            || lower.contains("shorter than")
            || lower.contains("max"))
}

fn lint_shape(lower: &str, text: &str) -> Option<(String, String)> {
    if (lower.contains("never `interface`") || lower.contains("not `interface`"))
        && lower.contains("`type`")
    {
        return Some((
            "@typescript-eslint/consistent-type-definitions".into(),
            r"^\s*(export\s+)?(declare\s+)?interface\s+".into(),
        ));
    }
    if lower.contains("console.log") || lower.contains("`console.") {
        return Some(("no-console".into(), r"console\.(log|debug|info)\(".into()));
    }
    if lower.contains("process.env") {
        return Some(("no-process-env".into(), r"process\.env\.".into()));
    }
    if lower.contains("date.now()") || lower.contains("`date.now`") {
        return Some(("no-restricted-syntax".into(), r"Date\.now\(".into()));
    }
    if lower.contains(" as ") && (lower.contains("cast") || lower.contains("no type casting") || lower.contains("`as`"))
    {
        return Some((
            "@typescript-eslint/consistent-type-assertions".into(),
            r"\bas\s+[A-Z]".into(),
        ));
    }
    if lower.contains("npm install") || lower.contains("yarn add") || lower.contains("pnpm add") {
        return Some(("no-restricted-syntax".into(), r"npm install|yarn add|pnpm add".into()));
    }
    if lower.contains(": any") || lower.contains("`any`") && lower.contains("type") {
        return Some(("@typescript-eslint/no-explicit-any".into(), r":\s*any\b".into()));
    }
    let _ = text;
    None
}

fn scaffold_question(text: &str) -> Question {
    let mut criteria = BTreeMap::new();
    criteria.insert("true".into(), format!("the file breaks: {text}"));
    criteria.insert("false".into(), format!("the file follows: {text}"));
    Question::Boolean {
        instructions: format!(
            "Does this file violate the project rule: \"{text}\"? Answer true only if the file as written breaks that rule."
        ),
        criteria: Some(criteria),
    }
}

fn same_rule(a: &str, b: &str) -> bool {
    normalize_words(a) == normalize_words(b)
}

fn normalize_words(text: &str) -> String {
    text.to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn rule_id(text: &str) -> String {
    let slug: String = text
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = slug.trim_start_matches(|c: char| c.is_ascii_digit() || c == '-');
    let mut slug: String = slug.chars().take(48).collect();
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "rule".into()
    } else {
        slug
    }
}

fn unique_id(text: &str, used: &mut HashSet<String>) -> String {
    let base = rule_id(text);
    if used.insert(base.clone()) {
        return base;
    }
    for n in 2..1000 {
        let next = format!("{base}-{n}");
        if used.insert(next.clone()) {
            return next;
        }
    }
    format!("{base}-x")
}

fn scope_globs(scope: &str) -> Option<Vec<String>> {
    if scope == "**/*" || scope.is_empty() {
        return None;
    }
    if scope.contains(',') && !scope.contains('{') {
        let parts: Vec<String> = scope
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts)
        }
    } else {
        Some(vec![scope.to_string()])
    }
}

pub fn rule_applies_to(rule: &Rule, path: &str) -> bool {
    match &rule.scope {
        None => true,
        Some(globs) => globs.iter().any(|g| glob_matches(g, path)),
    }
}

pub fn glob_matches(pattern: &str, path: &str) -> bool {
    let path = path.replace('\\', "/");
    expand_braces(pattern)
        .into_iter()
        .any(|p| glob_match(&p.replace('\\', "/"), &path))
}

fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(start) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    let Some(end) = pattern[start..].find('}') else {
        return vec![pattern.to_string()];
    };
    let end = start + end;
    let prefix = &pattern[..start];
    let suffix = &pattern[end + 1..];
    let inner = &pattern[start + 1..end];
    inner
        .split(',')
        .flat_map(|choice| expand_braces(&format!("{prefix}{choice}{suffix}")))
        .collect()
}

fn glob_match(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty() || pattern == "/").collect();
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    glob_segs(&pat, &segs)
}

fn glob_segs(pat: &[&str], segs: &[&str]) -> bool {
    match (pat.first().copied(), segs.first().copied()) {
        (None, None) => true,
        (Some("**"), _) => {
            if glob_segs(&pat[1..], segs) {
                return true;
            }
            if segs.is_empty() {
                return pat.iter().all(|p| *p == "**");
            }
            glob_segs(pat, &segs[1..])
        }
        (Some(_), None) => pat.iter().all(|p| *p == "**"),
        (None, Some(_)) => false,
        (Some(p), Some(s)) => star_match(p, s) && glob_segs(&pat[1..], &segs[1..]),
    }
}

fn star_match(pat: &str, seg: &str) -> bool {
    if pat == "*" || pat == seg {
        return true;
    }
    let p = pat.as_bytes();
    let s = seg.as_bytes();
    fn rec(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(b'*'), _) => rec(&p[1..], s) || (!s.is_empty() && rec(p, &s[1..])),
            (Some(b'?'), Some(_)) => rec(&p[1..], &s[1..]),
            (Some(a), Some(b)) if a == b => rec(&p[1..], &s[1..]),
            _ => false,
        }
    }
    rec(p, s)
}

pub fn pointer_target(text: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    if lines.is_empty() {
        return None;
    }
    if lines.len() > 3 {
        return None;
    }
    let joined = lines.join(" ");
    let lower = joined.to_ascii_lowercase();
    let rest = if let Some(r) = lower.strip_prefix("read instructions from ") {
        &joined[joined.len() - r.len()..]
    } else if let Some(r) = lower.strip_prefix("read ") {
        &joined[joined.len() - r.len()..]
    } else if let Some(r) = lower.strip_prefix("see ") {
        &joined[joined.len() - r.len()..]
    } else if let Some(r) = lower.strip_prefix('@') {
        &joined[joined.len() - r.len()..]
    } else {
        return None;
    };
    let rest = rest
        .trim()
        .trim_end_matches('.')
        .trim_matches('`')
        .trim_matches('"')
        .trim_matches('\'')
        .trim();
    if rest.is_empty() {
        return None;
    }
    if rest.contains(' ') {
        let token = rest.split_whitespace().last().unwrap_or(rest);
        if token.ends_with(".md") || token.ends_with(".txt") || token.contains('/') {
            return Some(token.trim_matches('`').trim_matches('"').to_string());
        }
        return None;
    }
    Some(rest.to_string())
}

fn frontmatter(text: &str) -> Option<String> {
    let text = text.trim_start_matches('\u{feff}');
    let rest = text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n"))?;
    let end = rest.find("\n---").or_else(|| rest.find("\r\n---"))?;
    Some(rest[..end].to_string())
}

fn strip_frontmatter(text: &str) -> &str {
    let trimmed = text.trim_start_matches('\u{feff}');
    if let Some(rest) = trimmed
        .strip_prefix("---\n")
        .or_else(|| trimmed.strip_prefix("---\r\n"))
    {
        if let Some(idx) = rest.find("\n---") {
            let after = &rest[idx + 4..];
            return after.strip_prefix('\n').unwrap_or(after);
        }
    }
    trimmed
}

fn front_bool(front: &str, key: &str) -> Option<bool> {
    for line in front.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.trim().trim_start_matches(':').trim();
            return match rest {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
        }
    }
    None
}

fn front_globs(front: &str) -> Option<Vec<String>> {
    let mut globs = Vec::new();
    let mut in_list = false;
    for line in front.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("globs:") {
            let rest = rest.trim();
            if rest.is_empty() {
                in_list = true;
                continue;
            }
            return Some(
                rest.split(',')
                    .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
            );
        }
        if in_list {
            if let Some(item) = trimmed.strip_prefix("- ") {
                globs.push(item.trim().trim_matches('"').trim_matches('\'').to_string());
            } else if !trimmed.is_empty() && !trimmed.starts_with('-') {
                break;
            }
        }
    }
    if globs.is_empty() {
        None
    } else {
        Some(globs)
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&sha256(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256(input: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state = [
        0x6a09e667u32, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (input.len() as u64).saturating_mul(8);
    let mut data = input.to_vec();
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in data.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, piece) in chunk.chunks_exact(4).enumerate().take(16) {
            w[i] = u32::from_be_bytes(piece.try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }
    let mut out = [0u8; 32];
    for (i, word) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn normalize_rel(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_string()
}

fn resolve_source(root: &Path, path: &str) -> PathBuf {
    let path = path.trim();
    if path.starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(&path[2..]);
        }
    }
    if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        root.join(path)
    }
}

fn utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = secs.div_euclid(86400);
    let tod = secs.rem_euclid(86400) as u32;
    let (y, m, d) = civil_from_days(days);
    let hh = tod / 3600;
    let mm = (tod % 3600) / 60;
    let ss = tod % 60;
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

pub fn band_for(probability: f32, thresholds: Thresholds) -> &'static str {
    if probability >= thresholds.act {
        "act"
    } else if probability >= thresholds.flag {
        "flag"
    } else {
        "clear"
    }
}

pub fn violation_probability(question: &Question, answer: &Value) -> (f32, Option<String>) {
    match question {
        Question::Boolean { .. } => {
            let p = answer
                .get("probability")
                .and_then(Value::as_f64)
                .or_else(|| {
                    answer.get("boolean").and_then(Value::as_bool).map(|b| {
                        if b {
                            1.0
                        } else {
                            0.0
                        }
                    })
                })
                .or_else(|| {
                    answer.get("answer").and_then(Value::as_bool).map(|b| {
                        if b {
                            1.0
                        } else {
                            0.0
                        }
                    })
                })
                .unwrap_or(0.0) as f32;
            (p.clamp(0.0, 1.0), None)
        }
        Question::Choice { violating, .. } => {
            let choice = answer
                .get("choice")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if let Some(probs) = answer.get("probabilities").and_then(Value::as_object) {
                let mass: f32 = violating
                    .iter()
                    .filter_map(|name| probs.get(name).and_then(Value::as_f64))
                    .sum::<f64>() as f32;
                return (mass.clamp(0.0, 1.0), Some(choice));
            }
            let p = if violating.iter().any(|v| v == &choice) {
                1.0
            } else {
                0.0
            };
            (p, Some(choice))
        }
        Question::Score {
            criteria,
            violating_from,
            ..
        } => {
            let score = answer.get("score").and_then(Value::as_f64).unwrap_or(0.0);
            let level = score.round().clamp(0.0, (criteria.len().saturating_sub(1)) as f64) as usize;
            let label = criteria.get(level).cloned().unwrap_or_else(|| level.to_string());
            if let Some(probs) = answer.get("probabilities").and_then(Value::as_object) {
                let mass: f32 = probs
                    .iter()
                    .filter_map(|(k, v)| {
                        let idx: usize = k.parse().ok()?;
                        if idx >= *violating_from {
                            v.as_f64().map(|p| p as f32)
                        } else {
                            None
                        }
                    })
                    .sum();
                return (mass.clamp(0.0, 1.0), Some(label));
            }
            let p = if score >= *violating_from as f64 {
                1.0
            } else {
                0.0
            };
            (p, Some(label))
        }
    }
}

pub fn verdicts_from_answers(
    answers: &Value,
    rules: &[&Rule],
    thresholds: Thresholds,
) -> Vec<RuleVerdict> {
    let mut out = Vec::new();
    for rule in rules {
        let Check::Model { question, .. } = &rule.check else {
            continue;
        };
        let key = format!("rule:{}", rule.id);
        let Some(answer) = answers.get(&key).or_else(|| answers.get(&rule.id)) else {
            continue;
        };
        let (probability, picked) = violation_probability(question, answer);
        let line = rule.source.line.unwrap_or(1);
        out.push(RuleVerdict {
            rule_id: rule.id.clone(),
            text: rule.text.clone(),
            source: format!("{}:{line}", rule.source.path),
            probability,
            band: band_for(probability, thresholds).to_string(),
            score: probability * 3.0,
            answer: picked,
        });
    }
    out.sort_by(|a, b| b.probability.total_cmp(&a.probability));
    out
}

pub fn questions_for_rules(base: Value, rules: &[&Rule]) -> Value {
    if rules.is_empty() {
        return base;
    }
    let mut questions = base;
    let Some(map) = questions.as_object_mut() else {
        return questions;
    };
    for rule in rules.iter().take(MAX_RULE_QUESTIONS) {
        if let Check::Model { question, .. } = &rule.check {
            map.insert(format!("rule:{}", rule.id), question.to_jev());
        }
    }
    questions
}

pub fn draft_notes(rules: &RulesFile, dest: &Path) -> String {
    let (model, lint, deferred, unenforceable) = rules.bucket_counts();
    format!(
        "Agent rules written to {} ({} rules: {model} model, {lint} lint, {deferred} deferred, {unenforceable} unenforceable).\n\
         \n\
         The compile is deterministic: it extracts instruction sentences and scaffolds a boolean\n\
         Jev question per model rule. To refine those questions, edit check.question on each\n\
         model rule in that file. A violating file should score near 1 and a clean file near 0.\n\
         Keep instructions under 60 words. Ask about existence in this file, not a judgment of\n\
         the whole. Do not add rules the instruction files do not state.",
        dest.display(),
        rules.rules.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tree(root: &Path, files: &[(&str, &str)]) {
        for (path, body) in files {
            let full = root.join(path);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(full, body).unwrap();
        }
    }

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("painpoints-rules-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const AGENTS: &str = "# Rules\n\
         \n\
         - Use Yup, never validate by hand.\n\
         - Never show a user a raw error.\n\
         - Ask when unsure.\n\
         - Keep files under 200 lines.\n\
         - Use `type`, never `interface`.\n\
         - Reuse existing error codes; don't invent near-duplicates.\n";

    #[test]
    fn discover_reads_documented_instruction_files() {
        let root = fixture("discover");
        write_tree(
            &root,
            &[
                ("AGENTS.md", "- Use Yup.\n"),
                ("CLAUDE.md", "Read AGENTS.md\n"),
                (".cursorrules", "- Prefer named exports.\n"),
                (".github/copilot-instructions.md", "- No default exports.\n"),
                (".cursor/rules/frontend.mdc", "---\nglobs: apps/web/**/*\n---\n- No inline styles.\n"),
                ("apps/web/AGENTS.md", "- Server files stay in src/server.\n"),
                ("apps/web/src/page.ts", "export {}\n"),
            ],
        );
        let found: Vec<String> = discover(&root).into_iter().map(|c| c.path).collect();
        assert!(found.contains(&"AGENTS.md".into()));
        assert!(found.contains(&"CLAUDE.md".into()));
        assert!(found.contains(&".cursorrules".into()));
        assert!(found.contains(&".github/copilot-instructions.md".into()));
        assert!(found.iter().any(|p| p.starts_with(".cursor/rules/")));
        assert!(found.contains(&"apps/web/AGENTS.md".into()));
    }

    #[test]
    fn nested_agents_are_scoped_to_their_directory() {
        let root = fixture("nested");
        write_tree(
            &root,
            &[
                ("AGENTS.md", "- Root rule.\n"),
                ("apps/web/AGENTS.md", "- Keep server handlers in src/server.\n"),
            ],
        );
        let found = discover(&root);
        let nested = found.iter().find(|c| c.path == "apps/web/AGENTS.md").unwrap();
        assert_eq!(nested.scope, "apps/web/**/*");
        let rules = compile(&root, &found);
        let nested_rule = rules
            .rules
            .iter()
            .find(|r| r.source.path == "apps/web/AGENTS.md")
            .expect("nested rule");
        assert_eq!(nested_rule.scope.as_deref(), Some(&["apps/web/**/*".into()][..]));
        assert!(rule_applies_to(nested_rule, "apps/web/src/page.ts"));
        assert!(!rule_applies_to(nested_rule, "apps/api/src/page.ts"));
    }

    #[test]
    fn pointer_files_are_followed_and_listed_as_sources() {
        let root = fixture("pointer");
        write_tree(
            &root,
            &[
                ("CLAUDE.md", "Read AGENTS.md\n"),
                ("AGENTS.md", "- Never log secrets.\n"),
            ],
        );
        let found = discover(&root);
        assert!(found.iter().any(|c| c.path == "AGENTS.md"));
        assert!(found.iter().any(|c| c.path == "CLAUDE.md"));
        assert_eq!(pointer_target("Read instructions from ./docs/RULES.md\n"), Some("./docs/RULES.md".into()));
        assert_eq!(pointer_target("# Claude\n\nRead AGENTS.md.\n"), Some("AGENTS.md".into()));
    }

    #[test]
    fn compile_extracts_quoted_rules_and_buckets_them() {
        let root = fixture("compile");
        write_tree(&root, &[("AGENTS.md", AGENTS)]);
        let rules = compile(&root, &discover(&root));
        assert_eq!(rules.version, 1);
        assert_eq!(rules.compiled_by.as_deref(), Some("painpoints"));
        assert_eq!(rules.sources[0].path, "AGENTS.md");
        assert!(rules.sources[0].sha.as_ref().unwrap().len() == 64);

        let by_text = |needle: &str| {
            rules
                .rules
                .iter()
                .find(|r| r.text.to_ascii_lowercase().contains(needle))
                .unwrap_or_else(|| panic!("missing {needle} in {:?}", rules.rules))
        };

        assert!(matches!(by_text("yup").check, Check::Model { .. }));
        assert!(matches!(by_text("raw error").check, Check::Model { .. }));
        assert!(matches!(by_text("ask when").check, Check::Unenforceable { .. }));
        assert!(matches!(by_text("200 lines").check, Check::Deferred { .. }));
        assert!(matches!(by_text("interface").check, Check::Lint { .. }));
        assert!(matches!(by_text("error codes").check, Check::Deferred { .. }));

        let yup = by_text("yup");
        assert_eq!(yup.text, "Use Yup, never validate by hand");
        assert_eq!(yup.source.line, Some(3));
        assert_eq!(yup.when.as_deref(), Some("edit"));
        assert!(yup.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
    }

    #[test]
    fn compile_adds_no_built_in_rules() {
        let root = fixture("empty");
        write_tree(&root, &[("README.md", "hello\n")]);
        assert!(discover(&root).is_empty());
        let rules = compile(&root, &[]);
        assert!(rules.rules.is_empty());
        assert!(rules.sources.is_empty());
    }

    #[test]
    fn rules_file_round_trips_and_old_json_without_optional_fields_loads() {
        let json = r#"{
            "version": 1,
            "compiledAt": "2026-09-19T00:00:00Z",
            "sources": [{"path": "AGENTS.md"}],
            "rules": [{
                "id": "use-yup",
                "text": "Use Yup, never validate by hand",
                "source": {"path": "AGENTS.md", "line": 3},
                "when": "edit",
                "check": {
                    "type": "model",
                    "question": {
                        "type": "boolean",
                        "instructions": "Does this file validate by hand?"
                    }
                }
            }]
        }"#;
        let rules: RulesFile = serde_json::from_str(json).unwrap();
        assert_eq!(rules.rules[0].status, "active");
        assert_eq!(rules.thresholds(), Thresholds::default());
        let again: RulesFile = serde_json::from_str(&serde_json::to_string(&rules).unwrap()).unwrap();
        assert_eq!(again.rules[0].id, "use-yup");
    }

    #[test]
    fn model_questions_are_injected_only_when_scope_matches() {
        let rule = Rule {
            id: "no-raw-error".into(),
            text: "Never show a user a raw error".into(),
            source: RuleSource {
                path: "AGENTS.md".into(),
                line: Some(4),
            },
            scope: Some(vec!["src/**/*.ts".into()]),
            when: Some("edit".into()),
            check: Check::Model {
                question: Question::Boolean {
                    instructions: "Does this file put raw exception text where a user will see it?".into(),
                    criteria: None,
                },
                overlaps: None,
            },
            status: "active".into(),
        };
        let rules = RulesFile {
            version: 1,
            compiled_at: "2026-09-19T00:00:00Z".into(),
            compiled_by: Some("painpoints".into()),
            sources: vec![],
            thresholds: None,
            rules: vec![rule],
        };
        assert_eq!(rules.model_rules_for("src/api.ts").len(), 1);
        assert!(rules.model_rules_for("README.md").is_empty());
        let applied = rules.applied_digest("src/api.ts");
        assert!(!applied.is_empty());
        assert!(rules.applied_digest("README.md").is_empty());

        let questions = questions_for_rules(json!({ "role": { "type": "choice" } }), &rules.model_rules_for("src/api.ts"));
        assert!(questions.get("rule:no-raw-error").is_some());
        assert_eq!(questions["rule:no-raw-error"]["type"], "boolean");
        let none = questions_for_rules(json!({ "role": { "type": "choice" } }), &[]);
        assert!(none.get("rule:no-raw-error").is_none());
    }

    #[test]
    fn digest_changes_when_a_matching_rule_changes() {
        let root = fixture("digest");
        write_tree(&root, &[("AGENTS.md", "- Never show a user a raw error.\n")]);
        let first = compile(&root, &discover(&root));
        write_tree(&root, &[("AGENTS.md", "- Never show a user a raw error.\n- Use Yup, never validate by hand.\n")]);
        let second = compile(&root, &discover(&root));
        assert_ne!(
            first.applied_digest("src/lib.rs"),
            second.applied_digest("src/lib.rs")
        );
    }

    #[test]
    fn staleness_tracks_source_hashes() {
        let root = fixture("stale");
        write_tree(&root, &[("AGENTS.md", "- Use Yup, never validate by hand.\n")]);
        let candidates = discover(&root);
        let rules = compile(&root, &candidates);
        assert!(!is_stale(&rules, &candidates, &root));
        std::fs::write(root.join("AGENTS.md"), "- Use Zod, never validate by hand.\n").unwrap();
        assert!(is_stale(&rules, &discover(&root), &root));
    }

    #[test]
    fn boolean_and_choice_answers_become_bands() {
        let question = Question::Boolean {
            instructions: "broken?".into(),
            criteria: None,
        };
        let (p, _) = violation_probability(&question, &json!({ "probability": 0.86 }));
        assert!((p - 0.86).abs() < 0.001);
        assert_eq!(band_for(p, Thresholds::default()), "act");
        assert_eq!(band_for(0.6, Thresholds::default()), "flag");
        assert_eq!(band_for(0.2, Thresholds::default()), "clear");

        let choice = Question::Choice {
            instructions: "shape".into(),
            criteria: BTreeMap::from([
                ("ok".into(), "fine".into()),
                ("bad".into(), "wrong".into()),
            ]),
            violating: vec!["bad".into()],
        };
        let (p, ans) = violation_probability(&choice, &json!({ "choice": "bad" }));
        assert_eq!(p, 1.0);
        assert_eq!(ans.as_deref(), Some("bad"));
    }

    #[test]
    fn sha256_matches_the_public_test_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn glob_matching_covers_double_star_and_braces() {
        assert!(glob_matches("**/*", "src/lib.rs"));
        assert!(glob_matches("apps/web/**/*", "apps/web/src/page.ts"));
        assert!(!glob_matches("apps/web/**/*", "apps/api/src/page.ts"));
        assert!(glob_matches("**/*.{ts,tsx}", "src/a.tsx"));
        assert!(glob_matches("src/**/*.rs", "src/rules.rs"));
        assert!(!glob_matches("src/**/*.rs", "docs/readme.md"));
    }

    #[test]
    fn turn_and_inactive_rules_are_not_judged_on_a_file() {
        let mut rule = Rule {
            id: "scope-creep".into(),
            text: "No features beyond what was asked".into(),
            source: RuleSource {
                path: "AGENTS.md".into(),
                line: Some(1),
            },
            scope: None,
            when: Some("turn".into()),
            check: Check::Model {
                question: Question::Boolean {
                    instructions: "scope?".into(),
                    criteria: None,
                },
                overlaps: None,
            },
            status: "active".into(),
        };
        let rules = RulesFile {
            version: 1,
            compiled_at: String::new(),
            compiled_by: None,
            sources: vec![],
            thresholds: None,
            rules: vec![rule.clone()],
        };
        assert!(rules.model_rules_for("src/a.rs").is_empty());
        rule.when = Some("edit".into());
        rule.status = "disabled".into();
        let rules = RulesFile {
            rules: vec![rule],
            ..rules
        };
        assert!(rules.model_rules_for("src/a.rs").is_empty());
    }
}
