---
name: painpoints-rules
description: Use when writing, refining or debugging the model questions in a repository's .painpoints/rules.json, after `painpoints compile`, when a report lists noisy_rules, when a project rule never fires or fires on almost every file, or when asked to make painpoints enforce the repo's AGENTS.md / CLAUDE.md rules better. Teaches how to phrase System One (Jev, Clef) questions so a violating file scores near 1 and a clean file near 0, and how to test them.
---

# Writing painpoints rules

`painpoints compile` turns the repository's instruction files into
`.painpoints/rules.json`. The compile is mechanical: every model rule gets a
scaffold that restates the instruction ("Does this file violate the project
rule: ...?"). A System One model answers that badly. It cannot see what the
rule is about, so it hovers around 0.5 on everything, or fires on every file
the rule does not fit. Your job is to turn each scaffold into a question the
model can answer from one window of one file.

The model reads one window of a file (up to 8000 characters, with
`start_line`, `end_line` and `path`) and answers every question
independently, with a probability per option. It does not reason in steps,
does not see other files, and does not know the repository. Write every
question for that reader.

## What the measurements say

These come from painpoints' own evaluation (`painpoints eval`) and a run on
40 files of a real service. Treat them as the rules of the craft:

1. **Existence beats degree.** "How much does this file mix layers?" caught 4
   of 10 planted problems on Clef-flash. "Is there at least one UI component
   that runs a database query?" caught 10 of 10 with no false alarms in 130
   checks. Ask whether a concrete construct exists, never how good the file
   is.
2. **Examples in the repository's own idiom.** An example written as Express
   `res.json({ message: String(error) })` scored the same mistake written as
   Hono `c.json({ error: error.message })` at 0.32. With a Hono example the
   same code scored 0.94. Copy the real shape from the codebase into
   `criteria`.
3. **The compliant example must not look like the violation.** A "correct"
   example of "a catch around parsing optional input that falls back to a
   default" made the model excuse a file that turned a failed fetch into
   `{}`: 0.84 fell to 0.25. Make the compliant example clearly different,
   for example `.catch(() => null)` followed by an explicit 400.
4. **Say when the question does not apply.** "...Answer no if this file
   defines no request handlers" took a false alarm on a file without
   handlers from 0.72 to 0.19. Without it the model guesses "yes" on files
   where the construct cannot occur.
5. **Scope is the biggest lever.** One table row from a `CLAUDE.md` about the
   desktop app's adapters, left unscoped, "failed" on 34 of 40 backend
   files. Every rule about part of the repository needs `scope` globs.

## The rule shape

```json
{
  "id": "no-raw-error-in-response",
  "text": "Never show a user a raw error.",
  "source": { "path": "AGENTS.md", "line": 120 },
  "scope": ["apps/backend/src/routes/**/*.ts"],
  "when": "edit",
  "check": {
    "type": "model",
    "question": {
      "type": "choice",
      "instructions": "Is there at least one response, page or notification in this file that includes an exception's own text, such as error.message or String(error), instead of a message written for people? Answer no if this file sends no responses.",
      "criteria": {
        "true": "Yes, for example: return c.json({ error: error instanceof Error ? error.message : String(error) }, 502)",
        "false": "No, for example: log.error(error); return c.json({ error: 'Could not load achievements. Try again.' }, 502)"
      },
      "violating": ["true"]
    }
  }
}
```

- `text` and `source` are the user's words and where they live. Never edit
  them and never invent a rule the instruction files do not state.
- `id` may be renamed to say what the rule catches. It is sent as the
  question name `rule.<id>`; painpoints reduces it to letters, digits, `_`,
  `.` and `-`, the only characters Clef accepts.
- `scope` is a list of globs relative to the repository root (`**`, `*`,
  `{a,b}`). Omit it only for rules that truly apply to every source file.
- `when: "turn"` rules are about a whole change ("no scope creep") and are
  not judged on a file. Leave them as they are.
- `status`: any value other than `active` (use `"off"`) keeps the rule in the
  file but stops asking it.
- Question types: `choice` with `true`/`false` criteria and
  `violating: ["true"]` for almost everything; `choice` with named options
  and a `violating` list when the rule names a closed set of shapes and some
  are wrong; `score` with ordered `criteria` levels (compliant first) and
  `violatingFrom` for a genuine matter of degree. Prefer `choice`.
- `thresholds` at the top level (`{"act": 0.8, "flag": 0.5}` by default)
  decide when a probability becomes a violation or a review flag. Do not
  lower them to make a weak question fire; fix the question.

## Refine a rules file

1. **Compile and read.** `painpoints compile` prints the bucket counts. Read
   the whole `rules.json` and the instruction files it came from.
2. **Fix the bucket first.** A model question is the wrong tool for:
   - anything a linter can check exactly (`interface`, `any`, `console.log`,
     import paths, `fetch(`): `"type": "lint"` with the lint rule in `how`;
   - counting or ordering (line limits, nesting depth, sorted imports):
     `"type": "deferred"`, `"reason": "needs a script, not a judge"`;
   - anything that needs the rest of the repository ("reuse existing
     helpers", "follow existing patterns"): `deferred` with a reason;
   - process and conversation ("ask first", "run the tests", "open a PR"):
     `"type": "unenforceable"` with a reason.
   Fragments that are not rules at all, such as table rows, headings or
   lists of file names, get `"status": "off"`.
3. **Turn off duplicates of the built-in detectors.** painpoints already asks
   every file about query in a loop, unbounded reads, swallowed errors,
   unsafe retries, injection, missing ownership checks, hardcoded secrets,
   raw errors shown to users, single-implementation abstractions and data
   access or business rules inside UI. A project rule that says the same
   thing costs tokens on every window and adds nothing; set it `"off"` and
   say in your summary which detector covers it.
4. **Scope every remaining model rule.** Find where the rule applies (the
   section heading of the instruction file usually says) and write globs
   for exactly those paths. A rule for `apps/desktop` must not see
   `apps/backend`.
5. **Rewrite each question** with the checklist below.
6. **Test each rewritten rule** (next section).
7. **Run the repository and read the noise.** `painpoints <repo> --headless`,
   then read `summary.by_rule` and `summary.noisy_rules` in
   `.painpoints/architecture-pain.json`. A rule in `noisy_rules` came back
   broken on half or more of at least ten files: rescope or rewrite it. A
   rule that never fires on a repository you know breaks it is too narrow.
   Read the top two or three hits of each rule in the code before calling
   it done. Revise each rule at most once more; if it still misbehaves, set
   it `"off"` and say why. A rule that is not trustworthy costs more than a
   missing one.

## Question checklist

- Starts with "Is there at least one ..." or "Does this file contain ..."
  and names a concrete construct: a call, a statement shape, an identifier,
  a value in a position.
- One idea. A rule with two parts becomes two rules with ids that say which
  part.
- States when the answer is no because the construct cannot occur ("Answer
  no if this file renders nothing").
- Under about 60 words. Every word is sent again for every window of every
  file in scope.
- `criteria.true` is a real violating line in the repository's own
  framework and naming, taken from or modelled on the codebase.
- `criteria.false` is the compliant way to do the same job, clearly
  different from the violation, not a near miss of it.
- No scope in the question text: paths belong in `scope`.
- No judgement words ("appropriate", "clean", "too much"); they average over
  the window and never leave the middle.
- A volume rule ("comment sparingly", "keep functions small") becomes an
  existence question for the concrete offence plus, if it really matters, a
  `score` question whose levels a reader could point at ("no such comments",
  "one or two", "a multi-line block", "most lines").

## Test a rule

Pick one file that you have read and know breaks the rule, and one in scope
that you know follows it (search the codebase for the construct). Then:

```
painpoints path/to/violating.ts --json
painpoints path/to/compliant.ts --json
```

Read `rule_verdicts` in each result: `ruleId`, `probability`, `band` and,
for long files, the `lines` of the window it came from. Changing a rule's
question invalidates the cache for the files it applies to, so no
`--refresh` is needed.

Aim for 0.8 or above on the violating file and 0.3 or below on the compliant
one. Between those, the question is ambiguous to the model: make the
construct more concrete, move the examples closer to the codebase's idiom,
or add the "answer no if" clause. A rule that scores high on both files is
asking about something the compliant file also contains.

Cost: each test classifies one file (a few thousand input tokens per
window). On Cloudflare's free plan that is well within a day's allowance;
running a whole large repository is not, so test on single files first and
use `--include` and `--limit` for the repository run.

## Edits are kept

Recompiling after an instruction file changes keeps every rule whose
instruction text is still there, with your id, scope, `when`, `status` and
question, and keeps `thresholds`. A rule whose instruction was removed goes
with it, and a reworded instruction comes back as a fresh scaffold to refine
again. Commit `rules.json`; it is meant to be reviewed and edited by hand.
Delete it to start from scaffolds.

## Report back

Tell the user, in a few sentences: how many rules are now model, lint,
deferred, unenforceable and off; which rules you scoped, rewrote, turned off
and why; the test probabilities for each rewritten rule; and any rule you
could not make reliable.
