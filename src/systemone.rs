use crate::decisions::{self, Decision};
use crate::rules::{self, RuleVerdict, RulesFile};
use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::LazyLock;

pub const DEFAULT_MODEL: &str = "jev-latest";
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
const SYSTEM_ONE_PATH: &str = "/v1/systemone";
pub const CLOUDFLARE_DEFAULT_MODEL: &str = "clef-flash";
pub const CLOUDFLARE_BASE_URL: &str = "https://api.cloudflare.com/client/v4";

/// Every provider speaks the System One request shape (`model`, `state`,
/// `questions` in, `answers` out); only the address, the credential and the
/// model name differ.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Provider {
    TypeSafe,
    Cloudflare,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn setting(name: &str, fallback: &str) -> String {
    env(name).unwrap_or_else(|| fallback.to_string())
}

/// `PAINPOINTS_PROVIDER` wins; otherwise whichever credential is present,
/// TypeSafe first so existing setups keep their behaviour.
pub fn provider() -> Provider {
    match env("PAINPOINTS_PROVIDER")
        .map(|p| p.to_ascii_lowercase())
        .as_deref()
    {
        Some("cloudflare") | Some("clef") => Provider::Cloudflare,
        Some(_) => Provider::TypeSafe,
        None if env("TYPESAFE_API_KEY").is_none() && env("CLOUDFLARE_API_TOKEN").is_some() => {
            Provider::Cloudflare
        }
        None => Provider::TypeSafe,
    }
}

fn configured_model(provider: Provider) -> String {
    match provider {
        Provider::TypeSafe => env("PAINPOINTS_MODEL")
            .unwrap_or_else(|| setting("TYPESAFE_DEFAULT_MODEL", DEFAULT_MODEL)),
        Provider::Cloudflare => setting("PAINPOINTS_MODEL", CLOUDFLARE_DEFAULT_MODEL),
    }
}

/// `clef-flash` or a full Workers AI id such as `@cf/cloudflare/clef-flash`;
/// the request body wants the bare name.
fn body_model(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or(model)
}

/// A larger model that re-answers only the detector questions the main model
/// was unsure about. `PAINPOINTS_CONFIRM_MODEL` sets it, `none` turns it off;
/// on Cloudflare it defaults to Clef behind Clef-flash, because Clef-flash
/// misses real bad decisions that Clef catches while the cascade costs a
/// fraction of running Clef on everything.
pub fn confirm_model() -> Option<String> {
    let provider = provider();
    match env("PAINPOINTS_CONFIRM_MODEL") {
        Some(m) if m.eq_ignore_ascii_case("none") || m.eq_ignore_ascii_case("off") => None,
        Some(m) => Some(m),
        None if provider == Provider::Cloudflare
            && body_model(&configured_model(provider)) == CLOUDFLARE_DEFAULT_MODEL =>
        {
            Some("clef".into())
        }
        None => None,
    }
}

/// The name reports and the cache digest record. Cloudflare models carry a
/// prefix so a Clef report is never reused as a Jev one, and a confirming
/// model is part of the name because it changes the answers.
pub fn model() -> String {
    let provider = provider();
    let model = configured_model(provider);
    let name = match provider {
        Provider::TypeSafe => model,
        Provider::Cloudflare => format!("cloudflare/{}", body_model(&model)),
    };
    match confirm_model() {
        Some(confirm) => format!("{name}+{}", body_model(&confirm)),
        None => name,
    }
}

pub fn provider_label() -> &'static str {
    match provider() {
        Provider::TypeSafe => "TypeSafe",
        Provider::Cloudflare => "Cloudflare Workers AI",
    }
}

fn typesafe_endpoint(base: &str) -> String {
    format!("{}{SYSTEM_ONE_PATH}", base.trim_end_matches('/'))
}

fn cloudflare_endpoint(base: &str, account: &str, model: &str) -> String {
    let id = if model.starts_with('@') {
        model.to_string()
    } else {
        format!("@cf/cloudflare/{model}")
    };
    format!(
        "{}/accounts/{account}/ai/run/{id}",
        base.trim_end_matches('/')
    )
}

pub struct Dimension {
    pub key: &'static str,
    pub label: &'static str,
    pub short: &'static str,
    pub question: &'static str,
    pub background: &'static str,
    pub note: &'static str,
    pub levels: [&'static str; 4],
    pub source: &'static str,
    pub url: &'static str,
}

pub const DIMENSIONS: [Dimension; 6] = [
    Dimension {
        key: "boundary_leak",
        label: "boundary leak",
        short: "bnd",
        question: "How much does this file mix responsibilities that belong to different layers of the application?",
        background: "A file is easy to change when it has one clear job at one layer. Cost rises when transport handling, business rules, persistence and presentation are interleaved in the same file, or when a rule that also lives elsewhere is restated here so the two copies can drift apart.",
        note: "Judge the responsibilities written in this file, not the quality of the names or the formatting.",
        levels: [
            "One clear responsibility at one layer; anything else is delegated to imported modules.",
            "Mostly one layer, with a small amount of adjacent glue such as mapping a fetched shape into the shape the caller wants.",
            "Two layers are meaningfully mixed: business rules inside a UI component or a request handler, query or storage details inlined into presentation, or validation rules restated next to logic that already assumes them.",
            "Transport, business rules, persistence and presentation are interleaved in one file, or it duplicates a rule that is also defined elsewhere so the copies will drift.",
        ],
        source: "Google Engineering Practices, What to look for in a code review",
        url: "https://google.github.io/eng-practices/review/reviewer/looking-for.html",
    },
    Dimension {
        key: "complexity",
        label: "complexity",
        short: "cpx",
        question: "How hard would it be for a developer who has not seen this file before to change it correctly?",
        background: "Reviewers ask whether the code could be simpler and whether another developer will understand it later. Over-engineering counts as complexity: code made more generic than the problem requires, or built for a need that does not exist yet.",
        note: "Judge the shape of the code, not its subject matter. Long but flat and repetitive code is easier than short but deeply conditional code.",
        levels: [
            "Short and linear; each unit does one obvious thing and the whole file fits in the reader's head.",
            "Some branching and a few moderately sized units, but the control flow is easy to follow and names carry the meaning.",
            "Long units, deep nesting, many parameters or boolean flags, or several responsibilities per unit, so the reader must hold a lot of state in mind.",
            "Deep nesting plus long units, or speculative indirection such as an abstraction with a single implementation, a factory for one product, or configuration for a value that never varies; changing it safely requires reading most of the file.",
        ],
        source: "Google Engineering Practices, complexity and over-engineering",
        url: "https://google.github.io/eng-practices/review/reviewer/looking-for.html",
    },
    Dimension {
        key: "data_access_cost",
        label: "data access cost",
        short: "data",
        question: "How expensive is the way this file reads or writes data from a database, external service or file store?",
        background: "Access cost is dominated by how many round trips are issued and how much data each one can return. A query issued inside a loop over the results of another query, an unbounded scan, sort or count over a whole table, and repeated identical calls with no reuse all grow with data volume and can overload the store long before the application looks slow.",
        note: "Judge only the access written in this file. If the file performs no data access at all, the first level applies.",
        levels: [
            "No database, network or file store access in this file.",
            "Bounded, keyed reads or writes: explicit fields, a narrow filter, a page size or a single record by identifier, with results reused rather than refetched.",
            "Fetches more than is needed, such as selecting all columns, omitting a limit, or issuing a fan-out of calls that grows with the size of a result set.",
            "A query inside a loop over another result set, an unbounded scan, sort or count across a whole table, or the same call repeated on every render or request with no caching or batching.",
        ],
        source: "Amazon Builders' Library, Caching challenges and strategies",
        url: "https://aws.amazon.com/builders-library/caching-challenges-and-strategies/",
    },
    Dimension {
        key: "failure_handling",
        label: "failure handling",
        short: "fail",
        question: "How likely is this file to turn a dependency's failure or slowness into a worse failure of its own?",
        background: "Remote calls need a timeout, a bounded number of retries, and backoff with jitter, otherwise retries synchronise into traffic spikes and slow dependencies pin resources until the caller collapses too. Retrying a non-idempotent request can duplicate work. Rarely exercised fallback paths tend to fail exactly when they are finally needed, and errors swallowed into a default value make a broken dependency look like an empty result.",
        note: "Judge only the calls written in this file. If the file makes no remote, database or file system calls, the first level applies.",
        levels: [
            "No remote, database or file system calls, or every call has an explicit timeout, a bounded retry policy and errors that reach the caller.",
            "Calls handle errors and propagate them, but a timeout or a concurrency bound is left to the library default.",
            "Retries without backoff or without a cap, unbounded parallel fan-out, a retried call that is not idempotent, or an error swallowed into a silent default value.",
            "A remote call with no timeout that is also retried, a fallback path that is never exercised in normal operation, or a catch-all that hides the failure so callers cannot tell an empty result from a broken dependency.",
        ],
        source: "Amazon Builders' Library, Timeouts, retries and backoff with jitter",
        url: "https://aws.amazon.com/builders-library/timeouts-retries-and-backoff-with-jitter/",
    },
    Dimension {
        key: "interaction_cost",
        label: "interaction cost",
        short: "ux",
        question: "How much does this file delay what the user sees or how fast the interface responds to input?",
        background: "Three effects dominate perceived quality in a browser: how long the largest content takes to appear, how long an interaction takes to produce the next frame, and how much the layout shifts after content loads. Work that blocks the main thread inside a render or an event handler delays the next frame. Fetching after a component mounts makes the parent finish before the child can start, so requests run in sequence instead of in parallel. Content inserted above existing content after load pushes it down.",
        note: "Judge only the rendering and interaction work written in this file. If the file never runs in a browser or renders no interface, the first level applies.",
        levels: [
            "Does not run in the browser, or renders nothing: no rendering or interaction work happens here.",
            "Renders from data the route or the parent already provides; per-interaction work is small and any list is short or virtualised.",
            "Starts its own fetch after mounting so requests run in sequence, recomputes an expensive derived value on every render, or renders a long list without virtualisation.",
            "Runs heavy synchronous work inside a render or an event handler without yielding, chains fetches from parent to child so each level waits for the one above, or inserts loaded content above existing content and shifts the layout.",
        ],
        source: "Google web.dev, Core Web Vitals (LCP, INP, CLS)",
        url: "https://web.dev/articles/vitals",
    },
    Dimension {
        key: "trust_boundary_risk",
        label: "trust boundary risk",
        short: "trust",
        question: "How exposed is this file where untrusted input, authorisation decisions or secrets meet the rest of the system?",
        background: "The most common serious defects are access control decisions a user can bypass, and untrusted input interpreted as code by a query, command, path, markup or redirect. Exposing a record by an identifier the client supplies, without checking the caller owns it, is the classic bypass.",
        note: "Judge only what this file does with input, authorisation and credentials. If it handles none of the three, the first level applies.",
        levels: [
            "Handles no external input, makes no authorisation decision and holds no credential.",
            "Handles external input but validates or parameterises it before use, and leaves authorisation to a shared, clearly named mechanism.",
            "Passes external input into a query, path, command or markup with only partial validation, or makes an authorisation decision inline and ad hoc alongside unrelated logic.",
            "Interpolates unvalidated input into a query, command, markup or redirect, returns or mutates a record by a client-supplied identifier with no ownership check, or embeds a credential in the source.",
        ],
        source: "OWASP Top 10:2021, A01 Broken Access Control and A03 Injection",
        url: "https://owasp.org/Top10/2021/A01_2021-Broken_Access_Control/",
    },
];

pub const PAIN_THRESHOLD: f32 = 2.0;

pub fn level_of(score: f32) -> usize {
    (score.round().max(0.0) as usize).min(3)
}

pub static QUESTIONS: LazyLock<Value> = LazyLock::new(|| {
    let mut questions = json!({
        "role": {
            "type": "choice",
            "instructions": "Which role does this file play in the application? Pick the one it spends most of its code on.",
            "criteria": {
                "ui-view": "Renders user interface: a page, screen, template, or presentational component.",
                "ui-state": "Client-side data and state glue: data-fetching hooks, stores, caches, context providers, API client wrappers, routing setup.",
                "api-surface": "The server's request surface: HTTP, RPC or GraphQL handlers, controllers, request validation, response shaping.",
                "domain-logic": "Business rules, calculations and workflows expressed independently of transport and storage details.",
                "data-access": "Talks to a database, external service or file store: queries, repositories, ORM models, storage and cache clients.",
                "platform": "Cross-cutting runtime plumbing: authentication, logging, configuration, middleware, background jobs, error handling, dependency wiring.",
                "contract": "Types, schemas, constants, generated clients or migrations: declarations with little or no behaviour.",
                "tooling-test": "Tests, fixtures, build scripts, codegen, benchmarks and developer tooling that does not ship to end users."
            }
        }
    });
    let map = questions.as_object_mut().expect("object");
    for dimension in DIMENSIONS {
        map.insert(
            dimension.key.to_string(),
            json!({
                "type": "score",
                "instructions": {
                    "question": dimension.question,
                    "background": dimension.background,
                    "note": dimension.note,
                },
                "criteria": dimension.levels,
            }),
        );
    }
    for detector in &decisions::DETECTORS {
        map.insert(decisions::key(detector.id), decisions::question(detector));
    }
    questions
});

static QUESTIONS_DIGEST: LazyLock<u64> = LazyLock::new(|| {
    let mut hasher = DefaultHasher::new();
    QUESTIONS.to_string().hash(&mut hasher);
    hasher.finish()
});

pub fn digest(windows: &[FileState], agent_rules: Option<&RulesFile>) -> String {
    let mut hasher = DefaultHasher::new();
    QUESTIONS_DIGEST.hash(&mut hasher);
    model().hash(&mut hasher);
    for state in windows {
        state.path.hash(&mut hasher);
        state.lines.hash(&mut hasher);
        state.start_line.hash(&mut hasher);
        state.end_line.hash(&mut hasher);
        state.truncated.hash(&mut hasher);
        state.source.hash(&mut hasher);
    }
    if let Some(applied) = windows
        .first()
        .zip(agent_rules)
        .map(|(state, r)| r.applied_digest(&state.path))
        .filter(|d| !d.is_empty())
    {
        applied.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

pub fn questions_for(path: &str, agent_rules: Option<&RulesFile>) -> Value {
    let Some(agent_rules) = agent_rules else {
        return QUESTIONS.clone();
    };
    rules::questions_for_rules(QUESTIONS.clone(), &agent_rules.model_rules_for(path))
}

/// One window of a source file as the model sees it. Files longer than one
/// window are read as several overlapping windows; `start_line` and
/// `end_line` say which part this is, and `truncated` marks the last window
/// when the file goes on past the window budget.
#[derive(Debug, Clone, Serialize)]
pub struct FileState {
    pub path: String,
    pub language: String,
    pub lines: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub truncated: bool,
    pub source: String,
}

#[derive(Debug, Deserialize)]
pub struct ChoiceAnswer {
    pub choice: String,
    pub confidence: f32,
}

#[derive(Debug, Deserialize)]
pub struct ScoreAnswer {
    pub score: f32,
    #[serde(default)]
    pub confidence: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct Answers {
    pub role: ChoiceAnswer,
    pub boundary_leak: ScoreAnswer,
    pub complexity: ScoreAnswer,
    pub data_access_cost: ScoreAnswer,
    pub failure_handling: ScoreAnswer,
    pub interaction_cost: ScoreAnswer,
    pub trust_boundary_risk: ScoreAnswer,
}

fn worse(a: ScoreAnswer, b: ScoreAnswer) -> ScoreAnswer {
    if b.score > a.score {
        b
    } else {
        a
    }
}

impl Answers {
    /// A file is as painful as its worst window; the role comes from the
    /// window that was surest about it.
    fn fold(self, other: Answers) -> Answers {
        Answers {
            role: if other.role.confidence > self.role.confidence {
                other.role
            } else {
                self.role
            },
            boundary_leak: worse(self.boundary_leak, other.boundary_leak),
            complexity: worse(self.complexity, other.complexity),
            data_access_cost: worse(self.data_access_cost, other.data_access_cost),
            failure_handling: worse(self.failure_handling, other.failure_handling),
            interaction_cost: worse(self.interaction_cost, other.interaction_cost),
            trust_boundary_risk: worse(self.trust_boundary_risk, other.trust_boundary_risk),
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

impl Usage {
    pub fn add(&mut self, other: Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
    }
}

fn round2<S: serde::Serializer>(value: &f32, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_f64(((*value as f64) * 100.0).round() / 100.0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scores {
    #[serde(serialize_with = "round2")]
    pub boundary_leak: f32,
    #[serde(serialize_with = "round2")]
    pub complexity: f32,
    #[serde(serialize_with = "round2")]
    pub data_access_cost: f32,
    #[serde(serialize_with = "round2")]
    pub failure_handling: f32,
    #[serde(serialize_with = "round2")]
    pub interaction_cost: f32,
    #[serde(serialize_with = "round2")]
    pub trust_boundary_risk: f32,
}

impl Scores {
    pub fn values(&self) -> [f32; 6] {
        [
            self.boundary_leak,
            self.complexity,
            self.data_access_cost,
            self.failure_handling,
            self.interaction_cost,
            self.trust_boundary_risk,
        ]
    }

    pub fn get(&self, key: &str) -> f32 {
        DIMENSIONS
            .iter()
            .position(|d| d.key == key)
            .map(|i| self.values()[i])
            .unwrap_or(0.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub path: String,
    pub lines: usize,
    pub role: String,
    #[serde(serialize_with = "round2")]
    pub role_confidence: f32,
    pub scores: Scores,
    pub worst_dimension: String,
    #[serde(serialize_with = "round2")]
    pub worst_score: f32,
    #[serde(serialize_with = "round2")]
    pub total_score: f32,
    pub needs_review: bool,
    pub digest: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<Decision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rule_verdicts: Vec<RuleVerdict>,
}

impl Record {
    /// Confirmed bad decisions and rule violations rank a file above any
    /// score, because they name something specific to fix.
    pub fn acted(&self) -> usize {
        self.decisions.iter().filter(|d| d.band == "act").count()
            + self
                .rule_verdicts
                .iter()
                .filter(|v| v.band == "act")
                .count()
    }

    pub fn rank_key(&self) -> (i32, i32, i32, &str) {
        (
            -(self.acted() as i32),
            -(self.worst_score * 1000.0) as i32,
            -(self.total_score * 1000.0) as i32,
            self.path.as_str(),
        )
    }

    pub fn has_rule_violation(&self) -> bool {
        self.rule_verdicts.iter().any(|v| v.band == "act")
    }

    pub fn has_bad_decision(&self) -> bool {
        self.decisions.iter().any(|d| d.band == "act")
    }

    pub fn is_pain_point(&self) -> bool {
        self.worst_score >= PAIN_THRESHOLD || self.has_rule_violation() || self.has_bad_decision()
    }
}

/// A rule needs to have been asked on this many files before its hit rate
/// says anything about the rule rather than about the files.
pub const NOISY_MIN_FILES: usize = 10;

/// A compiled rule that "fails" on half or more of the files it is asked
/// about is almost always a question the files cannot answer (a table row,
/// a rule about another app in the same repository), not a repository that
/// breaks its own rule everywhere. Its violations are banded `noisy` so they
/// stop ranking files and filling findings, and the rule is named so someone
/// can rewrite or scope it in rules.json. Recomputed over the whole report,
/// so a rule that stops misfiring gets its `act` verdicts back.
pub fn mark_noisy_rules(records: &mut [Record]) -> Vec<String> {
    let mut asked: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    for record in records.iter() {
        for verdict in &record.rule_verdicts {
            let entry = asked.entry(verdict.rule_id.clone()).or_default();
            entry.0 += 1;
            entry.1 += usize::from(verdict.band == "act" || verdict.band == "noisy");
        }
    }
    let noisy: Vec<String> = asked
        .into_iter()
        .filter(|(_, (files, hits))| *files >= NOISY_MIN_FILES && hits * 2 >= *files)
        .map(|(id, _)| id)
        .collect();
    for record in records.iter_mut() {
        for verdict in &mut record.rule_verdicts {
            let is_noisy = noisy.contains(&verdict.rule_id);
            if is_noisy && verdict.band == "act" {
                verdict.band = "noisy".into();
            } else if !is_noisy && verdict.band == "noisy" {
                verdict.band = "act".into();
            }
        }
    }
    noisy
}

pub fn to_record(state: &FileState, answers: &Answers) -> Record {
    let scores = Scores {
        boundary_leak: answers.boundary_leak.score,
        complexity: answers.complexity.score,
        data_access_cost: answers.data_access_cost.score,
        failure_handling: answers.failure_handling.score,
        interaction_cost: answers.interaction_cost.score,
        trust_boundary_risk: answers.trust_boundary_risk.score,
    };
    let values = scores.values();
    let worst =
        values.iter().enumerate().fold(
            (0usize, f32::MIN),
            |best, (i, &v)| {
                if v > best.1 {
                    (i, v)
                } else {
                    best
                }
            },
        );
    let confidences = [
        answers.boundary_leak.confidence,
        answers.complexity.confidence,
        answers.data_access_cost.confidence,
        answers.failure_handling.confidence,
        answers.interaction_cost.confidence,
        answers.trust_boundary_risk.confidence,
    ];
    let shaky_score = confidences
        .iter()
        .zip(values)
        .any(|(c, v)| v >= PAIN_THRESHOLD && c.is_some_and(|c| c < 0.5));

    Record {
        path: state.path.clone(),
        lines: state.lines,
        role: answers.role.choice.clone(),
        role_confidence: answers.role.confidence,
        worst_dimension: DIMENSIONS[worst.0].key.to_string(),
        worst_score: worst.1,
        total_score: values.iter().sum(),
        needs_review: answers.role.confidence < 0.6 || shaky_score,
        digest: digest(std::slice::from_ref(state), None),
        scores,
        decisions: Vec::new(),
        rule_verdicts: Vec::new(),
    }
}

/// Folds the raw answers for every window of one file into one record: the
/// worst score per dimension, and for each detector and rule the window
/// where it was most likely, with that window's lines when there was more
/// than one.
pub fn merge(
    windows: &[FileState],
    answers: &[Value],
    agent_rules: Option<&RulesFile>,
) -> Result<Record> {
    let Some(first) = windows.first() else {
        bail!("no windows to merge");
    };
    let mut folded: Option<Answers> = None;
    for raw in answers {
        let parsed: Answers = serde_json::from_value(raw.clone()).context("System One answers")?;
        folded = Some(match folded {
            None => parsed,
            Some(prev) => prev.fold(parsed),
        });
    }
    let Some(folded) = folded else {
        bail!("no answers for {}", first.path);
    };
    let mut record = to_record(first, &folded);
    record.digest = digest(windows, agent_rules);
    let located =
        |i: usize| (windows.len() > 1).then(|| [windows[i].start_line, windows[i].end_line]);

    for detector in &decisions::DETECTORS {
        let key = decisions::key(detector.id);
        let best = answers
            .iter()
            .enumerate()
            .filter_map(|(i, raw)| {
                raw.get(&key)
                    .and_then(decisions::yes_probability)
                    .map(|p| (i, p))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, probability)) = best {
            let confirmed_by = answers[i][&key]
                .get("confirmed_by")
                .and_then(Value::as_str)
                .map(str::to_string);
            record.decisions.push(Decision {
                id: detector.id.to_string(),
                dimension: detector.dimension.to_string(),
                label: detector.label.to_string(),
                probability,
                band: decisions::band_for(probability).to_string(),
                lines: located(i),
                confirmed_by,
            });
        }
    }
    record
        .decisions
        .sort_by(|a, b| b.probability.total_cmp(&a.probability));

    if let Some(agent_rules) = agent_rules {
        let matching = agent_rules.model_rules_for(&first.path);
        let mut best: Vec<RuleVerdict> = Vec::new();
        for (i, raw) in answers.iter().enumerate() {
            for mut verdict in
                rules::verdicts_from_answers(raw, &matching, agent_rules.thresholds())
            {
                verdict.lines = located(i);
                match best.iter_mut().find(|v| v.rule_id == verdict.rule_id) {
                    Some(seen) if seen.probability >= verdict.probability => {}
                    Some(seen) => *seen = verdict,
                    None => best.push(verdict),
                }
            }
        }
        best.sort_by(|a, b| b.probability.total_cmp(&a.probability));
        record.rule_verdicts = best;
    }
    if record.decisions.iter().any(|d| d.band == "flag")
        || record.rule_verdicts.iter().any(|v| v.band == "flag")
    {
        record.needs_review = true;
    }
    Ok(record)
}

/// Below this a detector answer is trusted as a no; at or above it the
/// confirming model is asked again.
pub const CONFIRM_FROM: f32 = 0.3;

struct Target {
    model: String,
    endpoint: String,
}

pub struct Client {
    http: reqwest::Client,
    key: String,
    main: Target,
    confirm: Option<Target>,
}

impl Client {
    pub fn new() -> Result<Self> {
        let provider = provider();
        let model = configured_model(provider);
        let confirm = confirm_model();
        let (key, endpoint, confirm_endpoint) = match provider {
            Provider::TypeSafe => (
                env("TYPESAFE_API_KEY").context(
                    "TYPESAFE_API_KEY is not set; create one at https://typesafe.ai, or set \
                     TYPESAFE_BASE_URL to a gateway and pass its token as the key. To use \
                     Cloudflare's Clef instead, set CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID",
                )?,
                typesafe_endpoint(&setting("TYPESAFE_BASE_URL", DEFAULT_BASE_URL)),
                confirm
                    .as_ref()
                    .map(|_| typesafe_endpoint(&setting("TYPESAFE_BASE_URL", DEFAULT_BASE_URL))),
            ),
            Provider::Cloudflare => {
                let token = env("CLOUDFLARE_API_TOKEN").context(
                    "CLOUDFLARE_API_TOKEN is not set; create an API token with the Workers AI \
                     permission in the Cloudflare dashboard",
                )?;
                let account = env("CLOUDFLARE_ACCOUNT_ID").context(
                    "CLOUDFLARE_ACCOUNT_ID is not set; `cf auth whoami` or the Workers AI page \
                     of the Cloudflare dashboard shows it",
                )?;
                let base = setting("CLOUDFLARE_BASE_URL", CLOUDFLARE_BASE_URL);
                (
                    token,
                    cloudflare_endpoint(&base, &account, &model),
                    confirm
                        .as_ref()
                        .map(|m| cloudflare_endpoint(&base, &account, m)),
                )
            }
        };
        let http = reqwest::Client::builder()
            .pool_max_idle_per_host(32)
            .timeout(std::time::Duration::from_secs(90))
            .build()?;
        Ok(Self {
            http,
            key,
            main: Target {
                model: body_model(&model).to_string(),
                endpoint,
            },
            confirm: confirm.zip(confirm_endpoint).map(|(m, endpoint)| Target {
                model: body_model(&m).to_string(),
                endpoint,
            }),
        })
    }

    async fn ask(
        &self,
        target: &Target,
        state: &FileState,
        questions: &Value,
    ) -> Result<(Value, Usage)> {
        let body = json!({ "model": target.model, "state": state, "questions": questions });
        let mut backoff = std::time::Duration::from_millis(500);
        for attempt in 0..4 {
            let res = self
                .http
                .post(&target.endpoint)
                .bearer_auth(&self.key)
                .json(&body)
                .send()
                .await?;
            let status = res.status();
            if status.is_success() {
                return parse_response(res.json().await?);
            }
            let retryable = status.as_u16() == 429 || status.is_server_error();
            if !retryable || attempt == 3 {
                let body = res.text().await.unwrap_or_default();
                bail!("{}", classify_http_error(status, &state.path, &body));
            }
            tokio::time::sleep(backoff).await;
            backoff *= 2;
        }
        unreachable!()
    }

    pub async fn classify(
        &self,
        windows: &[FileState],
        agent_rules: Option<&RulesFile>,
    ) -> Result<(Record, Usage)> {
        let Some(first) = windows.first() else {
            bail!("nothing to classify");
        };
        let questions = questions_for(&first.path, agent_rules);
        let replies = futures::future::try_join_all(
            windows.iter().map(|w| self.ask(&self.main, w, &questions)),
        )
        .await?;
        let mut usage = Usage::default();
        let mut answers = Vec::with_capacity(replies.len());
        for (raw, used) in replies {
            usage.add(used);
            answers.push(raw);
        }
        if let Some(confirm) = &self.confirm {
            usage.add(self.confirm_unsure(confirm, windows, &mut answers).await?);
        }
        Ok((merge(windows, &answers, agent_rules)?, usage))
    }

    /// Re-asks the confirming model only the detector questions each window
    /// answered at `CONFIRM_FROM` or above, and replaces those answers.
    async fn confirm_unsure(
        &self,
        confirm: &Target,
        windows: &[FileState],
        answers: &mut [Value],
    ) -> Result<Usage> {
        let mut jobs = Vec::new();
        for (i, raw) in answers.iter().enumerate() {
            let unsure: serde_json::Map<String, Value> = decisions::DETECTORS
                .iter()
                .map(|d| decisions::key(d.id))
                .filter(|key| {
                    raw.get(key)
                        .and_then(decisions::yes_probability)
                        .is_some_and(|p| p >= CONFIRM_FROM)
                })
                .map(|key| (key.clone(), QUESTIONS[&key].clone()))
                .collect();
            if !unsure.is_empty() {
                jobs.push((i, Value::Object(unsure)));
            }
        }
        let replies = futures::future::try_join_all(
            jobs.iter()
                .map(|(i, questions)| self.ask(confirm, &windows[*i], questions)),
        )
        .await?;
        let mut usage = Usage::default();
        for ((i, _), (confirmed, used)) in jobs.iter().zip(replies) {
            usage.add(used);
            if let (Some(target), Some(confirmed)) =
                (answers[*i].as_object_mut(), confirmed.as_object())
            {
                for (key, mut answer) in confirmed.clone() {
                    answer["confirmed_by"] = json!(confirm.model);
                    target.insert(key, answer);
                }
            }
        }
        Ok(usage)
    }
}

/// TypeSafe returns `{answers, usage}`; the Cloudflare REST API wraps the
/// same payload in `{result, success, errors}`.
fn parse_response(parsed: Value) -> Result<(Value, Usage)> {
    let payload = match parsed.get("result") {
        Some(inner) if inner.get("answers").is_some() => inner,
        _ => &parsed,
    };
    let answers = payload
        .get("answers")
        .cloned()
        .context("System One response has no answers")?;
    let usage = payload
        .get("usage")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    Ok((answers, usage))
}

pub fn classify_http_error(status: reqwest::StatusCode, path: &str, body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        format!(
            "HTTP {} classifying {path}: {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("request failed")
        )
    } else {
        format!("HTTP {} classifying {path}: {body}", status.as_u16())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(value: f32) -> ScoreAnswer {
        ScoreAnswer {
            score: value,
            confidence: Some(0.9),
        }
    }

    fn answers(role_conf: f32, values: [f32; 6]) -> Answers {
        Answers {
            role: ChoiceAnswer {
                choice: "data-access".into(),
                confidence: role_conf,
            },
            boundary_leak: score(values[0]),
            complexity: score(values[1]),
            data_access_cost: score(values[2]),
            failure_handling: score(values[3]),
            interaction_cost: score(values[4]),
            trust_boundary_risk: score(values[5]),
        }
    }

    fn state() -> FileState {
        FileState {
            path: "server/src/lib/fm.ts".into(),
            language: "typescript".into(),
            lines: 10,
            start_line: 1,
            end_line: 10,
            truncated: false,
            source: String::new(),
        }
    }

    #[test]
    fn worst_dimension_names_the_pain() {
        let record = to_record(&state(), &answers(0.9, [0.4, 1.1, 2.7, 1.0, 0.0, 0.3]));
        assert_eq!(record.worst_dimension, "data_access_cost");
        assert_eq!(record.worst_score, 2.7);
        assert!(!record.needs_review);
    }

    #[test]
    fn ranking_puts_the_worst_file_first() {
        let painful = to_record(&state(), &answers(0.9, [0.0, 0.0, 3.0, 0.0, 0.0, 0.0]));
        let broad = to_record(&state(), &answers(0.9, [1.9, 1.9, 1.9, 1.9, 1.9, 1.9]));
        assert!(painful.rank_key() < broad.rank_key());
    }

    #[test]
    fn low_confidence_is_flagged_only_where_it_changes_a_verdict() {
        let mut shaky = answers(0.9, [0.0, 0.0, 2.6, 0.0, 0.0, 0.0]);
        shaky.data_access_cost.confidence = Some(0.3);
        assert!(to_record(&state(), &shaky).needs_review);

        let mut harmless = answers(0.9, [0.0, 0.4, 0.0, 0.0, 0.0, 0.0]);
        harmless.complexity.confidence = Some(0.3);
        assert!(!to_record(&state(), &harmless).needs_review);

        assert!(to_record(&state(), &answers(0.4, [0.0; 6])).needs_review);
    }

    #[test]
    fn the_endpoint_is_built_from_the_base_url() {
        assert_eq!(
            typesafe_endpoint(DEFAULT_BASE_URL),
            "https://api.typesafe.ai/v1/systemone"
        );
        assert_eq!(
            typesafe_endpoint("https://gateway.example/typesafe/"),
            "https://gateway.example/typesafe/v1/systemone"
        );
    }

    #[test]
    fn cloudflare_endpoints_accept_a_bare_name_or_a_full_model_id() {
        assert_eq!(
            cloudflare_endpoint(CLOUDFLARE_BASE_URL, "acct", "clef-flash"),
            "https://api.cloudflare.com/client/v4/accounts/acct/ai/run/@cf/cloudflare/clef-flash"
        );
        assert_eq!(
            cloudflare_endpoint("https://gw.example/", "acct", "@cf/cloudflare/clef"),
            "https://gw.example/accounts/acct/ai/run/@cf/cloudflare/clef"
        );
        assert_eq!(body_model("@cf/cloudflare/clef"), "clef");
        assert_eq!(body_model("jev-latest"), "jev-latest");
    }

    #[test]
    fn responses_parse_bare_or_wrapped_in_a_cloudflare_result() {
        let bare = json!({"answers": {"role": {}}, "usage": {"input_tokens": 7}});
        let (answers, usage) = parse_response(bare.clone()).unwrap();
        assert!(answers.get("role").is_some());
        assert_eq!(usage.input_tokens, 7);

        let wrapped = json!({"result": bare, "success": true, "errors": []});
        let (answers, usage) = parse_response(wrapped).unwrap();
        assert!(answers.get("role").is_some());
        assert_eq!(usage.input_tokens, 7);

        assert!(parse_response(json!({"success": false})).is_err());
    }

    fn raw_answers(scores: [f32; 6], role_conf: f32, loop_yes: f32) -> Value {
        let mut answers = json!({
            "role": {"choice": "data-access", "confidence": role_conf},
        });
        for (dimension, score) in DIMENSIONS.iter().zip(scores) {
            answers[dimension.key] = json!({"score": score, "confidence": 0.9});
        }
        for detector in &decisions::DETECTORS {
            let p = if detector.id == "query_in_loop" {
                loop_yes
            } else {
                0.05
            };
            answers[decisions::key(detector.id)] = json!({"choice": if p >= 0.5 { "yes" } else { "no" }, "probabilities": {"yes": p, "no": 1.0 - p}});
        }
        answers
    }

    #[test]
    fn windows_merge_to_the_worst_score_and_locate_each_decision() {
        let mut first = state();
        first.lines = 300;
        first.end_line = 180;
        let mut second = first.clone();
        second.start_line = 160;
        second.end_line = 300;
        let record = merge(
            &[first, second],
            &[
                raw_answers([0.2, 1.0, 0.4, 0.0, 0.0, 0.0], 0.95, 0.1),
                raw_answers([0.1, 0.5, 2.6, 0.0, 0.0, 0.0], 0.6, 0.93),
            ],
            None,
        )
        .unwrap();
        assert_eq!(record.scores.complexity, 1.0);
        assert_eq!(record.scores.data_access_cost, 2.6);
        assert_eq!(record.worst_dimension, "data_access_cost");
        assert_eq!(record.role_confidence, 0.95);
        let looped = &record.decisions[0];
        assert_eq!(looped.id, "query_in_loop");
        assert_eq!(looped.band, "act");
        assert_eq!(looped.lines, Some([160, 300]));
        assert!(record.has_bad_decision() && record.is_pain_point());
        assert_eq!(record.decisions.len(), decisions::DETECTORS.len());
    }

    #[test]
    fn a_confirmed_bad_decision_ranks_above_a_higher_score() {
        let single = state();
        let decided = merge(
            std::slice::from_ref(&single),
            &[raw_answers([0.0, 0.0, 1.0, 0.0, 0.0, 0.0], 0.9, 0.9)],
            None,
        )
        .unwrap();
        assert_eq!(decided.decisions[0].lines, None);
        let scored = to_record(&single, &answers(0.9, [0.0, 0.0, 2.9, 0.0, 0.0, 0.0]));
        assert!(decided.rank_key() < scored.rank_key());
    }

    #[test]
    fn every_dimension_has_a_question_with_four_levels() {
        let keys = QUESTIONS.as_object().expect("object");
        assert!(keys.contains_key("role"));
        for dimension in DIMENSIONS {
            let question = keys
                .get(dimension.key)
                .unwrap_or_else(|| panic!("missing question {}", dimension.key));
            assert_eq!(question["type"], "score");
            assert_eq!(question["criteria"].as_array().unwrap().len(), 4);
            assert!(dimension.url.starts_with("https://"));
        }
        assert_eq!(DIMENSIONS.len(), 6);
        assert!(!keys.contains_key("agent_rule_compliance"));
        for detector in &decisions::DETECTORS {
            let question = &keys[&decisions::key(detector.id)];
            assert_eq!(question["type"], "choice");
            assert!(question["criteria"]["yes"]
                .as_str()
                .unwrap()
                .contains(detector.yes));
        }
        assert!(
            keys.len() + crate::rules::MAX_RULE_QUESTIONS <= 64,
            "Clef accepts at most 64 questions"
        );
        for key in keys.keys() {
            assert!(
                key.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')),
                "{key} is not a valid Clef question name"
            );
        }
        assert_eq!(
            crate::rules::question_key("no raw: error"),
            "rule.no-raw--error"
        );
    }

    fn model_rule(id: &str, scope: Option<Vec<String>>) -> rules::Rule {
        rules::Rule {
            id: id.into(),
            text: "Never show a user a raw error".into(),
            source: rules::RuleSource {
                path: "AGENTS.md".into(),
                line: Some(4),
            },
            scope,
            when: Some("edit".into()),
            check: rules::Check::Model {
                question: rules::Question::Boolean {
                    instructions: "Does this file put raw exception text where a user will see it?"
                        .into(),
                    criteria: None,
                },
                overlaps: None,
            },
            status: "active".into(),
        }
    }

    fn sample_rules(rules: Vec<rules::Rule>) -> RulesFile {
        RulesFile {
            version: 1,
            compiled_at: "2026-09-19T00:00:00Z".into(),
            compiled_by: Some("painpoints".into()),
            sources: vec![],
            thresholds: None,
            rules,
        }
    }

    #[test]
    fn questions_include_matching_model_rules_only() {
        let compiled = sample_rules(vec![model_rule(
            "no-raw-error",
            Some(vec!["server/**/*.ts".into()]),
        )]);
        let mut file = state();
        file.path = "server/src/routes/fm.ts".into();
        let with_rules = questions_for(&file.path, Some(&compiled));
        assert!(with_rules.get("rule.no-raw-error").is_some());
        assert_eq!(with_rules["rule.no-raw-error"]["type"], "choice");
        assert_eq!(
            with_rules["rule.no-raw-error"]["criteria"]["true"],
            "the file breaks the rule"
        );
        assert_eq!(
            with_rules["rule.no-raw-error"]["criteria"]["false"],
            "the file follows the rule"
        );
        for dimension in DIMENSIONS {
            assert_eq!(with_rules[dimension.key]["type"], "score");
        }

        file.path = "client/src/App.tsx".into();
        let without = questions_for(&file.path, Some(&compiled));
        assert!(without.get("rule.no-raw-error").is_none());
        assert_eq!(without, *QUESTIONS);
        assert_eq!(questions_for(&file.path, None), *QUESTIONS);
    }

    #[test]
    fn digest_is_stable_without_rules_and_moves_when_they_change() {
        let file = state();
        let empty = digest(std::slice::from_ref(&file), None);
        let unused = sample_rules(vec![model_rule(
            "no-raw-error",
            Some(vec!["apps/web/**/*".into()]),
        )]);
        assert_eq!(digest(std::slice::from_ref(&file), Some(&unused)), empty);

        let matching = sample_rules(vec![model_rule("no-raw-error", None)]);
        let first = digest(std::slice::from_ref(&file), Some(&matching));
        assert_ne!(first, empty);
        let changed = sample_rules(vec![model_rule("use-yup", None)]);
        assert_ne!(digest(std::slice::from_ref(&file), Some(&changed)), first);
    }

    #[test]
    fn old_record_json_without_rule_verdicts_deserializes() {
        let json = r#"{
            "path": "server/db.ts",
            "lines": 10,
            "role": "data-access",
            "role_confidence": 0.9,
            "scores": {
                "boundary_leak": 0.4,
                "complexity": 1.1,
                "data_access_cost": 2.7,
                "failure_handling": 1.0,
                "interaction_cost": 0.0,
                "trust_boundary_risk": 0.3
            },
            "worst_dimension": "data_access_cost",
            "worst_score": 2.7,
            "total_score": 5.5,
            "needs_review": false,
            "digest": "cafe"
        }"#;
        let record: Record = serde_json::from_str(json).unwrap();
        assert!(record.rule_verdicts.is_empty());
        assert_eq!(record.scores.data_access_cost, 2.7);
        assert!(!record.has_rule_violation());
    }

    #[test]
    fn classify_errors_include_status_and_response_body() {
        let status = reqwest::StatusCode::BAD_REQUEST;
        let err = classify_http_error(
            status,
            "src/lib.rs",
            r#"{"detail":{"error_type":"api_usage_error","message":"Invalid request."}}"#,
        );
        assert!(err.contains("HTTP 400"));
        assert!(err.contains("src/lib.rs"));
        assert!(err.contains("api_usage_error"));
        assert!(err.contains("Invalid request."));
        let bare = classify_http_error(status, "a.ts", "  ");
        assert_eq!(bare, "HTTP 400 classifying a.ts: Bad Request");
    }

    fn verdict(id: &str, band: &str) -> RuleVerdict {
        RuleVerdict {
            rule_id: id.into(),
            text: id.into(),
            source: "CLAUDE.md:1".into(),
            probability: if band == "act" { 0.9 } else { 0.1 },
            band: band.into(),
            score: 0.0,
            answer: None,
            lines: None,
        }
    }

    #[test]
    fn a_rule_that_fires_on_most_files_is_banded_noisy_and_can_recover() {
        let mut records: Vec<Record> = (0..12)
            .map(|i| {
                let mut r = to_record(&state(), &answers(0.9, [0.0; 6]));
                r.path = format!("src/f{i}.ts");
                r.rule_verdicts = vec![
                    verdict("table-row", if i < 9 { "act" } else { "clear" }),
                    verdict("real-rule", if i == 0 { "act" } else { "clear" }),
                ];
                r
            })
            .collect();
        assert_eq!(
            mark_noisy_rules(&mut records),
            vec!["table-row".to_string()]
        );
        assert_eq!(records[0].rule_verdicts[0].band, "noisy");
        assert_eq!(records[0].rule_verdicts[1].band, "act");
        assert!(records[0].has_rule_violation());
        assert!(!records[1].is_pain_point());

        for r in records.iter_mut().skip(1) {
            r.rule_verdicts[0].band = "clear".into();
        }
        assert!(mark_noisy_rules(&mut records).is_empty());
        assert_eq!(records[0].rule_verdicts[0].band, "act");

        let mut few = records[..3].to_vec();
        for r in &mut few {
            r.rule_verdicts[0].band = "act".into();
        }
        assert!(
            mark_noisy_rules(&mut few).is_empty(),
            "too few files to call a rule noisy"
        );
    }

    #[test]
    fn a_confirmed_answer_is_recorded_on_the_decision() {
        let mut raw = raw_answers([0.0; 6], 0.9, 0.2);
        raw[decisions::key("query_in_loop")] = json!({"choice": "yes", "probabilities": {"yes": 0.91, "no": 0.09}, "confirmed_by": "clef"});
        let record = merge(std::slice::from_ref(&state()), &[raw], None).unwrap();
        let looped = record
            .decisions
            .iter()
            .find(|d| d.id == "query_in_loop")
            .unwrap();
        assert_eq!(looped.confirmed_by.as_deref(), Some("clef"));
        assert_eq!(looped.band, "act");
        assert!(record
            .decisions
            .iter()
            .filter(|d| d.id != "query_in_loop")
            .all(|d| d.confirmed_by.is_none()));
    }
}
