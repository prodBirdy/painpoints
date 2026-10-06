use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// A concrete bad decision the model is asked to find. Each one is an
/// existence question ("is there at least one ...") with a worked example of
/// the mistake and of the fix, because a narrow question with an example
/// separates offending files from clean ones far better than asking how much
/// of a quality a whole file has. `evals/` measures exactly that separation.
pub struct Detector {
    pub id: &'static str,
    pub dimension: &'static str,
    pub label: &'static str,
    pub question: &'static str,
    pub yes: &'static str,
    pub no: &'static str,
}

pub const ACT: f32 = 0.75;
pub const FLAG: f32 = 0.5;

pub const DETECTORS: [Detector; 10] = [
    Detector {
        id: "query_in_loop",
        dimension: "data_access_cost",
        label: "query inside a loop",
        question: "Is there at least one database query or remote call issued inside a loop over the results of another query or collection, so the number of round trips grows with the data?",
        yes: "for (const order of orders) { await db.query('SELECT ... WHERE order_id = $1', [order.id]) }",
        no: "one query with a JOIN or WHERE id IN (...) that fetches everything at once",
    },
    Detector {
        id: "unbounded_read",
        dimension: "data_access_cost",
        label: "unbounded read",
        question: "Is there at least one read that loads a whole table or collection with no LIMIT, page size or narrowing filter, so its cost grows with all the data ever stored?",
        yes: "db.query('SELECT * FROM events ORDER BY created_at')",
        no: "SELECT id, name FROM users WHERE team_id = $1 LIMIT 50",
    },
    Detector {
        id: "swallowed_error",
        dimension: "failure_handling",
        label: "error swallowed into a default",
        question: "Is there at least one catch block or error callback that neither rethrows, logs nor reports the error, and instead returns null, undefined, false, an empty value or a default, so a failed database, network or file call looks the same to the caller as 'not found' or 'empty'?",
        yes: "try { const row = await db.selectFrom('subs').where('customer_id', '=', id).executeTakeFirst(); return row?.user_id ?? null } catch { return null }, or try { return await (await fetch(url)).json() } catch { return {} }",
        no: "catch (err) { log.error(err); throw err }, or const body = await req.json().catch(() => null) followed by if (!body) return c.json({ error: 'expected a JSON body' }, 400)",
    },
    Detector {
        id: "unsafe_retry",
        dimension: "failure_handling",
        label: "unsafe retry",
        question: "Is there at least one remote call that is retried without a delay that grows between attempts, or retried with no timeout, or a request that charges, creates or sends something being retried without an idempotency key?",
        yes: "for (let i = 0; i < 10; i++) { try { return await fetch(url, { method: 'POST' }) } catch {} }",
        no: "a single call with an AbortSignal timeout, or retries with exponential backoff and jitter on an idempotent GET",
    },
    Detector {
        id: "injection",
        dimension: "trust_boundary_risk",
        label: "untrusted input interpolated",
        question: "Is there at least one place where input from a request, user or other external source is concatenated or interpolated into an SQL query, shell command, file path, HTML or redirect URL instead of being passed as a parameter or escaped?",
        yes: "conn.execute(f\"SELECT * FROM users WHERE name LIKE '%{name}%'\")",
        no: "conn.execute('SELECT * FROM users WHERE name LIKE ?', (f'%{name}%',))",
    },
    Detector {
        id: "missing_ownership_check",
        dimension: "trust_boundary_risk",
        label: "record fetched without an ownership check",
        question: "Is there at least one HTTP or RPC request handler in this file that reads, changes or deletes a stored record by an identifier taken from the request path, query or body, without checking that the record belongs to the signed-in user or account? Answer no if this file defines no request handlers.",
        yes: "router.delete('/invoices/:id', requireSession, ...) calling invoices.deleteById(req.params.id)",
        no: "invoices.deleteForAccount(req.session.accountId, req.params.id), or a file with no request handlers",
    },
    Detector {
        id: "hardcoded_secret",
        dimension: "trust_boundary_risk",
        label: "secret in source",
        question: "Does this file contain a literal password, API key, token or private key written into the source, rather than read from the environment or a secret store?",
        yes: "auth: { user: 'mailer', pass: 'Winter-Sunset-2024!' }",
        no: "auth: { pass: process.env.SMTP_PASSWORD }",
    },
    Detector {
        id: "raw_error_to_user",
        dimension: "trust_boundary_risk",
        label: "raw error shown to a user",
        question: "Is there at least one place where an exception's own text (error.message, String(error), err.toString() or a stack trace) is put into an HTTP response body, page or notification that a client or user receives, instead of a message the code wrote for people?",
        yes: "return c.json({ error: error instanceof Error ? error.message : String(error) }, 502), or res.status(500).json({ message: String(error) })",
        no: "log.error(error); return c.json({ error: 'Could not load achievements. Try again.' }, 502)",
    },
    Detector {
        id: "speculative_abstraction",
        dimension: "complexity",
        label: "abstraction with one implementation",
        question: "Is there at least one interface, factory, strategy or configuration point that has only one implementation or one value in this file and exists for variation that never happens?",
        yes: "a ChannelFactory whose create(kind) returns EmailChannel for every kind, used only to create EmailChannel",
        no: "a plain function or class used directly, or an interface with several real implementations",
    },
    Detector {
        id: "layer_mixing",
        dimension: "boundary_leak",
        label: "data access or business rule inside UI",
        question: "Does a user-interface component in this file run database queries or contain business rules such as pricing, permissions or eligibility, instead of receiving them from a separate module or the server?",
        yes: "a React component that calls pool.query(...) and computes seats * 12 * discount inline",
        no: "a component that renders props or data loaded by a hook that calls an API",
    },
];

pub fn key(id: &str) -> String {
    format!("bad.{id}")
}

pub fn question(detector: &Detector) -> Value {
    json!({
        "type": "choice",
        "instructions": detector.question,
        "criteria": {
            "yes": format!("Yes. For example: {}", detector.yes),
            "no": format!("No. Correct code looks like: {}", detector.no),
        },
    })
}

/// One detector's verdict on a file: the highest probability over every
/// window the file was read in, and the lines of that window.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Decision {
    pub id: String,
    pub dimension: String,
    pub label: String,
    pub probability: f32,
    pub band: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<[usize; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed_by: Option<String>,
}

pub fn band_for(probability: f32) -> &'static str {
    if probability >= ACT {
        "act"
    } else if probability >= FLAG {
        "flag"
    } else {
        "clear"
    }
}

pub fn yes_probability(answer: &Value) -> Option<f32> {
    if let Some(p) = answer
        .get("probabilities")
        .and_then(|probs| probs.get("yes"))
        .and_then(Value::as_f64)
    {
        return Some((p as f32).clamp(0.0, 1.0));
    }
    answer.get("choice").and_then(Value::as_str).map(|c| {
        if c.eq_ignore_ascii_case("yes") {
            1.0
        } else {
            0.0
        }
    })
}

pub fn detector(id: &str) -> Option<&'static Detector> {
    DETECTORS.iter().find(|d| d.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_detector_belongs_to_a_dimension_and_ids_are_unique() {
        let dims: Vec<&str> = crate::systemone::DIMENSIONS.iter().map(|d| d.key).collect();
        let mut ids = std::collections::HashSet::new();
        for d in DETECTORS {
            assert!(
                dims.contains(&d.dimension),
                "{} has unknown dimension",
                d.id
            );
            assert!(ids.insert(d.id), "duplicate detector {}", d.id);
        }
    }

    #[test]
    fn yes_probability_reads_the_distribution_or_falls_back_to_the_choice() {
        let full = json!({"choice": "no", "probabilities": {"yes": 0.31, "no": 0.69}});
        assert_eq!(yes_probability(&full), Some(0.31));
        assert_eq!(yes_probability(&json!({"choice": "yes"})), Some(1.0));
        assert_eq!(yes_probability(&json!({})), None);
        assert_eq!(band_for(0.8), "act");
        assert_eq!(band_for(0.6), "flag");
        assert_eq!(band_for(0.2), "clear");
    }
}
