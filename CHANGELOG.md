# Changelog

## 0.3.0 — 2026-10-06

Minor release: any System One model, and findings that name concrete bad
decisions instead of only scoring how much of a quality a file has.

### Models

- Cloudflare Workers AI is a provider beside TypeSafe: `CLOUDFLARE_API_TOKEN`
  and `CLOUDFLARE_ACCOUNT_ID` select Clef, `PAINPOINTS_PROVIDER` picks when
  both credentials are set, and `PAINPOINTS_MODEL` overrides the model for
  either. Existing `TYPESAFE_*` setups behave as before.
- The Cloudflare `{result, success}` envelope is unwrapped.
- On Cloudflare, Clef-flash answers everything and Clef re-answers any
  bad-decision question Clef-flash put at 0.3 or above
  (`PAINPOINTS_CONFIRM_MODEL`, `none` to turn it off). Decisions answered
  that way carry `confirmed_by`.
- Rule questions are sent as `rule.<id>` (was `rule:<id>`): Clef only accepts
  question names made of letters, digits, `_`, `.` and `-`. Answers keyed the
  old way are still read.
- The model module is `systemone`, not `jev`.

### Bad decisions

- Ten existence questions with worked examples: query in a loop, unbounded
  read, swallowed error, unsafe retry, injection, missing ownership check,
  hardcoded secret, raw error shown to a user, speculative abstraction and
  data access or business rules inside UI. A detector at 0.75 or above is a
  `bad:<id>` finding; files with confirmed decisions or rule violations rank
  first. `summary.by_decision` counts them.
- `painpoints eval` runs fourteen labelled files built into the binary
  through the configured model and prints how well each detector separates
  the planted problem from clean code, next to how many the dimension scores
  catch.

### Coverage and noise

- Long files are read whole, in overlapping windows of up to 8000 characters
  (at most six), instead of only their first 8000 characters. Scores take the
  worst window; decisions and rule verdicts carry the `lines` of the window
  they were found in.
- A compiled rule that comes back broken on half or more of at least ten
  files is banded `noisy`, listed in `summary.noisy_rules` and the Markdown
  report, and left out of findings and ranking.
- The Markdown report's file table no longer ends its separator row with an
  extra `|`.

## 0.2.0 — 2026-09-19

Minor release: compile the target repository's own agent-instruction files
into `.painpoints/rules.json` and judge matching rules beside the six
architecture dimensions. Also the bugfixes that made that path usable on
worktrees and against TypeSafe SystemOne.

### Agent rules

- `painpoints compile [TARGET]` discovers `AGENTS.md`, `CLAUDE.md`, `AGENT.md`,
  `AGENTS.txt`, `.cursorrules`, `.cursor/rules/**/*.{md,mdc}`,
  `.github/copilot-instructions.md`, `.claude/CLAUDE.md`, nested `AGENTS.md` /
  `CLAUDE.md` (scoped to that subtree), and pointer files, then writes a
  hand-editable `.painpoints/rules.json`.
- Classify, MCP and the viewer load that file (or compile it if missing or
  stale against source hashes). Active `model` rules whose `scope` matches the
  file become extra Jev questions on the same SystemOne call as the six
  dimensions.
- Violations appear in `findings` as `rule:<id>`, quoting the instruction and
  pointing at its source line. They sit beside architecture findings; they do
  not replace the six dimensions.
- There are no built-in extra rules. Lint rules are recorded only. `when: turn`
  rules stay in `rules.json` and are not judged on a whole-file classify.

### Bugfixes

- **#2** Model questions are TypeSafe `choice` (true/false criteria,
  `violating: ["true"]`), not `boolean`. SystemOne has no boolean type. Legacy
  `boolean` entries in a hand-edited `rules.json` are mapped to that shape
  before they are sent.
- **#3** Scan walks a git worktree whose `.git` is a file, not a directory.
- **#4** Discover skips gitignored `.claude/worktrees` / `.cursor/worktrees`,
  and on duplicate rules keeps the broader scope so a nested copy cannot steal
  the root instruction file.
- **#5** Process and conversation rules compile as `unenforceable` and do not
  count against the model-rule caps. The per-file question budget prefers
  path-scoped rules. Compile prints which sources were truncated.

Windows x64 binary is attached on the GitHub release, as with 0.1.0. Other
platforms build with `cargo build --release`.

## 0.1.0 — 2026-09-17

First release.

- Six architecture dimensions with levels written from published guidance:
  Google eng-practices, Google SRE, web.dev, the Amazon Builders' Library and
  OWASP. Every finding carries the URL of the standard it was judged against.
- Three entry points sharing one cache: `painpoints <file> --json`,
  `painpoints <repo> --json`, and `painpoints mcp` (`painpoints_file` /
  `painpoints_repo`).
- The JSON report is the cache. Unchanged files cost no tokens; a fully cached
  run needs no API key.
- Native viewer (GPUI) with per-dimension distribution, heat cells and a panel
  that explains which level each score landed on.
- Claude Code skill under `.claude/skills/painpoints`.
