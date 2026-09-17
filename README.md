# painpoints

Point it at a repository. It scores every source file on six architectural pain
dimensions and writes a ranked report an AI coding agent can read before it
touches anything.

The dimensions are not invented. Each one's levels describe situations that
Google, Amazon and OWASP have published warnings about, so a finding can be
checked against its source instead of taken on trust.

```
$ painpoints ~/myapp --headless

classifying 189 files
  189/189
189 files (0 reused), 40 pain points, 631247 in / 33074 out tokens
~/myapp/.painpoints/architecture-pain.json
~/myapp/.painpoints/ARCHITECTURE-PAIN.md
```

```
| file                                  | role        | worst            | score | bnd | cpx | data | fail | ux  | trust |
| ------------------------------------- | ----------- | ---------------- | ----- | --- | --- | ---- | ---- | --- | ----- |
| server/src/routes/admin.ts            | api-surface | boundary_leak    | 2.8   | 2.8 | 1.4 | 1.1  | 1.1  | 0.0 | 2.1   |
| client/src/providers/AuthProvider.tsx | ui-state    | failure_handling | 2.7   | 0.9 | 0.5 | 1.0  | 2.7  | 1.7 | 1.3   |
| server/src/routes/fm.ts               | api-surface | failure_handling | 2.6   | 2.4 | 2.4 | 2.4  | 2.6  | 0.1 | 1.4   |
```

Without `--headless` the same run opens a window for browsing and filtering the
result.

## Install

```
cargo install --git https://github.com/prodBirdy/painpoints
```

or clone and `cargo build --release`. Needs a [TypeSafe](https://typesafe.ai)
API key in `TYPESAFE_API_KEY`; the judgments come from their Jev model, which
returns typed answers and calibrated probabilities rather than prose.

```
painpoints [TARGET] [options]
painpoints mcp

  TARGET            a repository to classify, or a single source file
                    (default: PAINPOINTS_ROOT or the current directory)
  --include DIR     only walk this subdirectory, repeatable
  --limit N         stop after N files
  --out DIR         where the report is written (default: ROOT/.painpoints)
  --headless        write the report without opening a window
  --json            print the result to stdout as JSON
  --refresh         reclassify everything instead of reusing the saved report

  mcp               serve the Model Context Protocol on stdio
```

It walks any language it recognises (TypeScript, JavaScript, Python, Go, Rust,
Java, Kotlin, C#, PHP, Ruby, Swift, Scala, Elixir, Dart, Vue, Svelte, Astro,
SQL), respects `.gitignore`, and skips vendored, generated and minified files.

## What it scores

Each file gets one `role` and six scores from 0 (healthy) to 3 (painful). 2.0
and above counts as a pain point. Files rank by their worst dimension first,
then by how many dimensions hurt.

| dimension | the question it answers | standard behind the levels |
| --- | --- | --- |
| `boundary_leak` | Does this file mix responsibilities from different layers, or restate a rule that lives elsewhere? | [Google, What to look for in a code review](https://google.github.io/eng-practices/review/reviewer/looking-for.html) |
| `complexity` | How hard is it for a new reader to change this correctly? Over-engineering counts. | [Google, complexity and over-engineering](https://google.github.io/eng-practices/review/reviewer/looking-for.html) |
| `data_access_cost` | How many round trips, and how much can each return? Query-in-a-loop, unbounded scans, refetch per render. | [Amazon Builders' Library, Caching challenges and strategies](https://aws.amazon.com/builders-library/caching-challenges-and-strategies/) |
| `failure_handling` | Will a slow or failing dependency become a worse failure here? Missing timeouts, retries without backoff, swallowed errors, untested fallbacks. | [Timeouts, retries and backoff with jitter](https://aws.amazon.com/builders-library/timeouts-retries-and-backoff-with-jitter/), [Avoiding fallback in distributed systems](https://aws.amazon.com/builders-library/avoiding-fallback-in-distributed-systems/), [Google SRE, Addressing cascading failures](https://sre.google/sre-book/addressing-cascading-failures/) |
| `interaction_cost` | Does this delay what the user sees or how fast the UI answers input? Main-thread blocking, fetch waterfalls, layout shift. | [web.dev, Core Web Vitals](https://web.dev/articles/vitals), [Optimize long tasks](https://web.dev/articles/optimize-long-tasks), [React, You Might Not Need an Effect](https://react.dev/learn/you-might-not-need-an-effect) |
| `trust_boundary_risk` | What does this file do with untrusted input, authorisation and secrets? | [OWASP Top 10:2021 A01 Broken Access Control](https://owasp.org/Top10/2021/A01_2021-Broken_Access_Control/), [A03 Injection](https://owasp.org/Top10/2021/A03_2021-Injection/) |

The criteria sent to the model describe those situations in plain terms, with
no framework or vendor names, so the scores mean the same thing in a Django
service as in a Next.js app.

## Saved results

The JSON report is also the cache. Every record carries a `digest` over the
question set and the exact state sent for that file, so a rerun only calls the
API for files whose digest changed: edit three files in a 300 file repo and the
next run costs three requests. `--refresh` reclassifies everything, and
changing a question invalidates every digest by itself. With nothing to
reclassify the run needs no API key at all, so reopening the window to browse
the last result is free.

## For agents

Three ways in, all sharing one cache.

**One file, atomic.** Point it at a single file and get that file's result on
stdout. Nothing else is read, nothing else is written.

```
$ painpoints src/routes/admin.ts --json
{
  "path": "src/routes/admin.ts",
  "role": "api-surface",
  "scores": { "boundary_leak": 2.8, "complexity": 1.4, "data_access_cost": 1.1,
              "failure_handling": 1.1, "interaction_cost": 0.0, "trust_boundary_risk": 2.1 },
  "worst_dimension": "boundary_leak",
  "worst_score": 2.8,
  "is_pain_point": true,
  "findings": [
    {
      "dimension": "boundary_leak",
      "score": 2.8,
      "level": 3,
      "description": "Transport, business rules, persistence and presentation are interleaved in one file, or it duplicates a rule that is also defined elsewhere so the copies will drift.",
      "source": "Google Engineering Practices, What to look for in a code review",
      "url": "https://google.github.io/eng-practices/review/reviewer/looking-for.html"
    }
  ],
  "cached": false,
  "tokens": { "input_tokens": 4446, "output_tokens": 175 }
}
```

`findings` is the part worth acting on: one entry per dimension at 2.0 or
above, carrying the level the score landed on, its description, and the
published standard behind it. A healthy file returns an empty `findings` array,
which is a real answer rather than a shrug.

**The whole codebase.** `painpoints . --json` returns the same shape as the
saved report: `summary.by_dimension` for the distribution, `files` ranked worst
first, `dimensions` with a `url` each.

**As a tool call.** `painpoints mcp` serves the Model Context Protocol on
stdio with two tools:

| tool | arguments | returns |
| --- | --- | --- |
| `painpoints_file` | `path`, `refresh?` | the atomic result above |
| `painpoints_repo` | `root?`, `include?`, `limit?`, `top?`, `refresh?` | distribution, the worst `top` files with their findings, and where the report was written |

```json
{
  "mcpServers": {
    "painpoints": {
      "command": "painpoints",
      "args": ["mcp"],
      "env": { "TYPESAFE_API_KEY": "sk-..." }
    }
  }
}
```

Scores are model judgments over the first 8000 characters of a file. They are a
reading order, not evidence. Anything flagged `needs_review` is where the model
itself was unsure, and a file whose interesting code starts past 8000
characters is judged on what came before it.

## Licence

MIT
