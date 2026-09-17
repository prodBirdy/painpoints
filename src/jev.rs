use anyhow::{bail, Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::LazyLock;

pub const DEFAULT_MODEL: &str = "jev-latest";
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
const SYSTEM_ONE_PATH: &str = "/v1/systemone";

fn setting(name: &str, fallback: &str) -> String {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

pub fn model() -> String {
    setting("TYPESAFE_DEFAULT_MODEL", DEFAULT_MODEL)
}

pub fn endpoint() -> String {
    let base = setting("TYPESAFE_BASE_URL", DEFAULT_BASE_URL);
    format!("{}{SYSTEM_ONE_PATH}", base.trim_end_matches('/'))
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
    questions
});

static QUESTIONS_DIGEST: LazyLock<u64> = LazyLock::new(|| {
    let mut hasher = DefaultHasher::new();
    QUESTIONS.to_string().hash(&mut hasher);
    hasher.finish()
});

pub fn digest(state: &FileState) -> String {
    let mut hasher = DefaultHasher::new();
    QUESTIONS_DIGEST.hash(&mut hasher);
    model().hash(&mut hasher);
    state.path.hash(&mut hasher);
    state.lines.hash(&mut hasher);
    state.truncated.hash(&mut hasher);
    state.source.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[derive(Debug, Clone, Serialize)]
pub struct FileState {
    pub path: String,
    pub language: String,
    pub lines: usize,
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

#[derive(Debug, Default, Deserialize, Serialize, Clone, Copy)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct SystemOneResult {
    answers: Answers,
    #[serde(default)]
    usage: Usage,
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
}

impl Record {
    pub fn rank_key(&self) -> (i32, i32, &str) {
        (
            -(self.worst_score * 1000.0) as i32,
            -(self.total_score * 1000.0) as i32,
            self.path.as_str(),
        )
    }
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
    let worst = values
        .iter()
        .enumerate()
        .fold((0usize, f32::MIN), |best, (i, &v)| {
            if v > best.1 {
                (i, v)
            } else {
                best
            }
        });
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
        digest: digest(state),
        scores,
    }
}

pub struct Client {
    http: reqwest::Client,
    key: String,
    model: String,
    endpoint: String,
}

impl Client {
    pub fn new() -> Result<Self> {
        let key = std::env::var("TYPESAFE_API_KEY")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .context("TYPESAFE_API_KEY is not set; create one at https://typesafe.ai, or set TYPESAFE_BASE_URL to a gateway and pass its token as the key")?;
        let http = reqwest::Client::builder()
            .pool_max_idle_per_host(32)
            .timeout(std::time::Duration::from_secs(90))
            .build()?;
        Ok(Self {
            http,
            key,
            model: model(),
            endpoint: endpoint(),
        })
    }

    pub async fn classify(&self, state: &FileState) -> Result<(Record, Usage)> {
        let body = json!({ "model": self.model, "state": state, "questions": &*QUESTIONS });
        let mut backoff = std::time::Duration::from_millis(500);
        for attempt in 0..4 {
            let res = self
                .http
                .post(&self.endpoint)
                .bearer_auth(&self.key)
                .json(&body)
                .send()
                .await?;
            let status = res.status();
            if status.is_success() {
                let parsed: SystemOneResult = res.json().await?;
                return Ok((to_record(state, &parsed.answers), parsed.usage));
            }
            let retryable = status.as_u16() == 429 || status.is_server_error();
            if !retryable || attempt == 3 {
                bail!(
                    "{} from {}: {}",
                    status,
                    self.endpoint,
                    res.text().await.unwrap_or_default()
                );
            }
            tokio::time::sleep(backoff).await;
            backoff *= 2;
        }
        unreachable!()
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
            format!("{}{SYSTEM_ONE_PATH}", DEFAULT_BASE_URL.trim_end_matches('/')),
            "https://api.typesafe.ai/v1/systemone"
        );
        assert_eq!(
            format!("{}{SYSTEM_ONE_PATH}", "https://gateway.example/typesafe/".trim_end_matches('/')),
            "https://gateway.example/typesafe/v1/systemone"
        );
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
    }
}
