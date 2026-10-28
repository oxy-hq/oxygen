# Builder knowledge module

This directory holds reference documents that are **compile-time embedded**
into the `agentic-builder` crate via `include_str!` (see
[`src/prompts.rs`](../src/prompts.rs)). Every document here is either a
verbatim copy of a file in Oxy's platform skills
([`sdk/cli/skills/`](../../../../sdk/cli/skills/), published as
[`oxy-hq/skills`](https://github.com/oxy-hq/skills)) or an authored
condensation of one. The skills are the **source of truth**; this directory
exists to put that knowledge inside the builder binary.

## Contents

| File                            | Condensed or copied from (`sdk/cli/skills/…`)                          | Shape    |
| ------------------------------- | ---------------------------------------------------------------------- | -------- |
| `semantic-layer-reference.md`   | `oxy-semantic-modeling/SKILL.md`                                       | authored |
| `view-template.yml`             | `oxy-semantic-modeling/templates/view-template.yml`                    | verbatim |
| `topic-template.yml`            | `oxy-semantic-modeling/templates/topic-template.yml`                   | verbatim |
| `agentic-builder-reference.md`  | `oxy-agents/SKILL.md` + `references/quick-reference.md`                | authored |
| `agentic-template.yml`          | `oxy-agents/templates/agentic-template.yml`                            | verbatim |

The file names keep their old spelling (`semantic-layer`) because
`prompts.rs` and its tests name them; the skills they come from were renamed when
they moved into this repository.

## Provenance frontmatter on authored cards

Each `*-reference.md` carries machine-readable provenance in YAML
frontmatter at the top of the file:

```yaml
---
source:
  - sdk/cli/skills/<skill>/SKILL.md
  - sdk/cli/skills/<skill>/references/quick-reference.md
reconciled-hash: <git blob hash of the sources, concatenated in order>
note: |
  Free-form note for human maintainers.
---
```

The frontmatter is **stripped** by `strip_vendoring_header()` in
`src/prompts.rs` before the card is fenced into the system prompt, so
the LLM never sees it. Treat the fields as the contract:

- `source:` — repo-relative paths of the skill files the card condenses.
- `reconciled-hash:` — what `scripts/check-skills-drift.sh --hash <card>`
  printed when the card was last re-condensed. `pending` means it never has
  been against these sources, which the drift check reports as behind.

## Do not edit by hand

These files feed the builder agent's system prompts. Editing one here
creates drift between the skills and the prompts the builder ships with.
Instead:

1. Edit the skill under `sdk/cli/skills/`, in the same PR as the behaviour
   it describes.
2. For a template, run `./scripts/sync-skills.sh` from the repo root; it
   copies the verbatim YAML back into this directory.
3. For an authored card, re-condense it from its sources, then record
   `reconciled-hash:` as printed by
   `./scripts/check-skills-drift.sh --hash crates/agentic/builder/knowledge/<card>`.
   Keep each card under ~200 lines so the LLM context stays focused.

## Drift detection

`scripts/check-skills-drift.sh` compares each card's `reconciled-hash` with
the hash of its sources today, and each template with its source byte for
byte. It needs no network and no history. Exit code is `0` when everything
is current, `1` on drift, `2` on a broken setup (a listed source that no
longer exists, say).

```bash
./scripts/check-skills-drift.sh
```

`.github/workflows/skills-reconcile.yaml` runs it:
- on PRs that touch this directory, the sync scripts, or `sdk/cli/skills/`
  (informational `check` job — it never blocks; a skill edit that leaves a
  card behind is expected, and the reconcile job catches it up), and
- after every merge to `main` that touches `sdk/cli/skills/` — it follows
  the Skills publish run — plus a weekly safety-net cron, via the
  `reconcile` job, which re-condenses the behind
  cards, re-syncs the templates, and opens a PR for human review.

## Why compile-time embedding

- **Reproducibility** — every builder binary ships with a pinned copy of
  the knowledge it was built against. No runtime filesystem lookup, no
  cross-repository coupling at deploy time.
- **Offline robustness** — the builder still works in sandboxes that
  cannot reach the skills repo or the network.
- **Reviewability** — changes to the knowledge module show up in the same
  PR as the code change that depends on them.

## When to grow this module

Add a new file here when a builder phase starts duplicating non-trivial
domain knowledge inside its prompt string. Good signals:

- You're about to inline a YAML template into a `format!` macro.
- You're about to re-document a rule that already lives in `sdk/cli/skills/`.
- Two different phases are each restating the same constraint in their
  own words.
