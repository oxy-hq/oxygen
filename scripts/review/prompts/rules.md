## Scope: this PR's diff, and what its own changes can break

In scope: the lines this PR adds or modifies, plus their direct side effects: callers
and callees of changed code, and the persisted, wire or config contracts those edits can
violate. Read as much surrounding code as you need to judge those lines, but every
finding must be caused by this diff: name the changed line it comes from.

Out of scope, however true it may be:
- pre-existing bugs, debt or style in code this PR did not touch
- "while you're here" refactors, renames or extra abstraction
- features, tests or docs the PR never set out to add
- repo-wide patterns the PR merely happens to follow
- anything CI already fails on: compile errors, clippy, rustfmt, biome, oxlint, failing tests

This review re-runs on every push. Reaching for new, unrelated code after the author
fixes what was flagged is what makes a PR never converge. On an incremental review, look
at what changed since the last reviewed head, and at whether those changes introduced
defects; earlier findings are tracked separately and must not be reported again.

## Severity: pick it by consequence, not by how sure you are

- `blocking`: merging causes harm that is hard to undo. Data loss or corruption; a
  security or authorization hole; one tenant's data reachable by another; a crash or
  outage on a common path; a migration or deploy that breaks a running older binary; a
  HARD rule in CLAUDE.md broken in a way no test catches.
- `should_fix`: a real defect someone will hit. A wrong result on a reachable path; a
  regression of existing behaviour; a failure that is swallowed or misreported; a
  request-path performance problem (a full table scan or unbounded query per request,
  N+1 queries, work that grows with tenant data on a hot path); a broken wire, config or
  API contract; risky logic with no test at all.
- `nit`: true and worth knowing, but no behaviour changes. Never style, naming or
  formatting.

A performance problem on a request path is never a nit.

## Each finding

- `reasoning` comes first. Name or quote the changed line, say what it does, and give the
  concrete input or sequence of events that makes it fail. If you cannot describe the
  failure, you do not have a finding.
- `file` is the path from the repository root. `line` is the line number in the head
  version of that file (the checkout), on or next to the changed line.
- `confidence`: 0.9 or above when you traced the failure path in the code; 0.6 to 0.8
  when it depends on something you could not confirm (say what in `reasoning`); below
  0.5, do not report it.
- `fix`: the concrete change, in a sentence or a short snippet.
- `suggestion`: only when the whole fix is new text for lines `line` to `end_line` of
  this file (set `end_line` when it is more than one line), and those lines are in the
  diff. Give the exact replacement for those lines, indentation included, with no code
  fence. The author commits it in one click, so leave it out when the fix touches any
  other line or file, needs a decision, or you are not sure it compiles. A finding
  without one is fine; a wrong one costs the author's trust in the next.
- `rule`: when the finding breaks written repo guidance, name it (`CLAUDE.md:
  Authorization`, `oxy-data-placement`, `R-M2`). Otherwise leave it empty.
- `category`: the closest listed category.

Report at most {{maxFindings}} findings, worst first. Fewer defended findings beat many
plausible ones: every false alarm teaches the team to skip the next real one.

`not_reviewed`: the files or areas in scope you did not read closely enough to vouch
for. An honest list here is part of a finished review; leave it empty only when you read
everything.
