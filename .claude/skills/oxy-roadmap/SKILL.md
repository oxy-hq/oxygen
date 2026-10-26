---
name: oxy-roadmap
description: Use when a PR in oxy-hq/oxygen-internal adds or changes product behaviour (a new endpoint, page, CLI command or flag, app power, changed default or contract, a feature landing), when the `roadmap` merge status fails, when editing internal-docs/platform-roadmap.md or its one-page summary, when picking or pinning a roadmap line id, or when writing the `Roadmap:` trailer of a PR body. Triggers on "roadmap", "Roadmap:", "roadmap status", "which roadmap line", "mark landed", "platform-roadmap.md", "roadmap summary", "rm:", "line id". Not for the roadmap app itself (oxy-hq/oxygen-roadmap).
---

# The roadmap moves with the PR

`internal-docs/platform-roadmap.md` is the company-facing roadmap: Now / Next / Later lines
under three audiences, sorted by confidence. Its Governance section is the rule; the
`roadmap` merge status (`scripts/review/src/roadmap.ts`) checks the shape of it on every
PR, and the automated review judges the substance. This skill is the mechanics.

## Decide: does this PR move a line?

Ask it before the PR body is written, not after the status fails.

| The PR | Trailer | Doc edit |
| --- | --- | --- |
| lands a Now line (the feature is in) | `Roadmap: <id>` | prefix the line with `Landed:`, cite the PR |
| starts a Next line (the first PR of it) | `Roadmap: <id>` | move the line under **Now**, cite the PR |
| adds behaviour no line names | `Roadmap: <new-id>` | add a line under **Now** (a PR exists) with a bold headline anyone can read, pin it, cite the PR |
| contributes to a Now line already under way | `Roadmap: <id>` | cite the PR on the line |
| fixes a bug, refactors, tests, bumps a dependency, changes an internal tool | `Roadmap: none -- <why>` (or no trailer: the review's general pass then says whether behaviour changed, and the status follows it) | none |
| is typed `chore:`, `docs:`, `test:`, `ci:`, `build:` or `style:` (a release bump, a template, CI) | none needed: the type says no behaviour changes | none |
| edits the roadmap's prose, pins or scoreboard reading only | `Roadmap: none -- <why>` (or no trailer) | regenerate the summary |

A reworded, moved or landed line is a move: the PR must name it. A `none` on a PR that
visibly adds behaviour is flagged by the review as `should_fix`; be honest, the line is
cheap.

## The commands

```bash
node scripts/roadmap/roadmap.ts ids                               # every line id, by audience and horizon
node scripts/roadmap/roadmap.ts link --id <id> --pr <n> --write    # cite PR n on the line (+ its link definition)
node scripts/roadmap/roadmap.ts pin --write                        # pin every unpinned (new) line to its id
node scripts/roadmap/roadmap.ts summary --write                    # regenerate platform-roadmap-summary.md
node scripts/roadmap/roadmap.ts check --body-file body.md --pr <n> --title "<PR title>"  # what the status will say
```

`check` reads the PR title's type (`feat:`, `chore:`, …) from `--title`; without it, the
last commit's subject stands in, which can differ from the PR's.

Without `--write` an edit prints as a diff. The PR number exists only once the PR is
open: open it, then `link` and push, or let the first push fail the status and fix it in
the second. Run `summary --write` after any edit to the roadmap; the status fails when the
summary is not derived from the roadmap at the PR head.

## Writing a line

- `  - **Headline anyone can read.** The detail, with the PR cited as ([#n]).` under the
  right `- **Now**` / `- **Next**` / `- **Later**` list of the right `### 2.x` audience.
  Continuation lines are indented four spaces; keep lines under ~92 columns.
- Landed lines start with `Landed:` before the bold headline. They stay until the quarterly
  review prunes them.
- The pin comment `    <!-- rm:<id> -->` is the last line of the item, and every line has
  one: the status fails a roadmap change that leaves a line unpinned. `pin --write` writes
  it from the headline's first words; never rename a pin, the PRs linked to it would be lost.
- PR link definitions live at the foot of the doc under `<!-- PR links -->`; `link` adds them.
- The summary is never edited by hand.

## The trailer

The last line of the PR body: `Roadmap: <id>` (several: `Roadmap: a-line, another-line`),
or `Roadmap: none -- <why>` with a real reason (at least a few words). The PR template
carries an empty `Roadmap: ` line; an empty value fails the status. A `feat`/`fix`/
`refactor`/`perf` PR on product code with no trailer at all is not failed on the spot: the
status waits (`pending`) for the automated review of that head, whose general pass answers
whether the diff changes behaviour and which line it belongs on, then passes on
"unchanged" or fails naming the line on "changed". Writing the trailer settles it without
the wait, and is the answer when the review is wrong. Several `Roadmap:`
lines add up. The roadmap app (oxy-hq/oxygen-roadmap) reads the same trailer to show the
PR on its board, label it `roadmap`, and flag drift (a Now line whose PRs all merged, a
Landed line whose PR is still open).
