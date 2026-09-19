---
name: painpoints
description: Use when asked where the architectural pain points, technical debt or risky files in a codebase are, which files to refactor first, whether a file breaks the repo's own AGENTS.md / CLAUDE.md rules, or to check a file for layering, complexity, data access cost, failure handling, UI cost or trust-boundary risk before or after editing it. Runs the painpoints tool and reads its JSON.
---

# painpoints

Scores source files 0 to 3 on six architectural dimensions with TypeSafe's Jev
model. Every dimension's levels describe situations published by Google,
Amazon and OWASP, and every finding carries the URL of that standard. Use it
to decide what to read and what to touch first, not as proof that something is
wrong.

## Pick the entry point

1. **MCP tools present** (`painpoints_file`, `painpoints_repo`): call them.
2. **Otherwise the CLI**, which prints the same JSON:
   - one file: `painpoints path/to/file.ts --json`
   - a repository: `painpoints path/to/repo --json`
   - a saved report already exists at `<repo>/.painpoints/architecture-pain.json`; read it before running anything.

The JSON report is the cache. Unchanged files cost no tokens, so rerunning
after an edit is cheap: only the edited files are reclassified. Pass
`refresh` only when the question set, model, or `.painpoints/rubric.json`
changed. `painpoints compile` writes that rubric from AGENTS.md and friends
without calling the model.

## Read a file result

```json
{
  "path": "src/routes/admin.ts",
  "role": "api-surface",
  "scores": { "boundary_leak": 2.8, "complexity": 1.4, "data_access_cost": 1.1,
              "failure_handling": 1.1, "interaction_cost": 0.0, "trust_boundary_risk": 2.1 },
  "worst_dimension": "boundary_leak",
  "worst_score": 2.8,
  "is_pain_point": true,
  "needs_review": false,
  "findings": [
    { "dimension": "boundary_leak", "score": 2.8, "level": 3,
      "description": "Transport, business rules, persistence and presentation are interleaved in one file, or it duplicates a rule that is also defined elsewhere so the copies will drift.",
      "source": "Google Engineering Practices, What to look for in a code review",
      "url": "https://google.github.io/eng-practices/review/reviewer/looking-for.html" }
  ]
}
```

- `findings` is the actionable part: one entry per architecture dimension at
  2.0 or above, plus one entry per agent-rule violation (`dimension` starts
  with `rule:`). An empty array means the file is healthy on every dimension
  and every compiled project rule; say so rather than inventing concerns.
- `is_pain_point` is `worst_score >= 2.0`.
- `needs_review` means the model itself was unsure. Read the file before
  acting on its scores.
- Scores cover the first 8000 characters of a file. A file whose interesting
  code starts later was judged on what came before it.

## Read a repository result

- `summary.by_dimension` says what kind of pain the repo has. Lead with the
  dimension that has the most files, and name the standard behind it.
- `files` (report) or `top` (MCP) is ranked worst dimension first, then by how
  many dimensions hurt. Work top down.
- `summary.by_role` shows where the pain lives structurally, for example
  `api-surface` handlers scoring high on `boundary_leak` means business rules
  sit in route handlers.

## Dimensions

| key | it measures | standard |
| --- | --- | --- |
| `boundary_leak` | layers mixed in one file, rules duplicated across files | Google eng-practices, code review design |
| `complexity` | how hard a new reader finds it to change safely; over-engineering counts | Google eng-practices, complexity |
| `data_access_cost` | round trips and result size: N+1, unbounded scans, refetch per render | Amazon Builders' Library, caching |
| `failure_handling` | timeouts, retries with backoff, swallowed errors, untested fallbacks | Amazon Builders' Library, timeouts and retries; Google SRE ch. 22 |
| `interaction_cost` | main-thread blocking, fetch waterfalls, layout shift | web.dev Core Web Vitals; React docs |
| `trust_boundary_risk` | unvalidated input into queries or markup, ad hoc authorisation, embedded secrets | OWASP Top 10 A01, A03 |

## After you change a file

Run `painpoints <file> --json` (or `painpoints_file`) again. Report the
before and after scores for the dimensions you touched. A drop below 2.0 on
the dimension you targeted is the success criterion; a rise elsewhere is a
regression to mention.

## Do not

- Quote a score as evidence of a defect. It is a reading order.
- Reclassify a whole repository to answer a question about one file.
- Hide `needs_review`. It changes how much weight the scores carry.
