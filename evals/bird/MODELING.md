# Authoring a BIRD semantic model

Authoring rules (one `<db>/` directory of `*.view.yml` files per database).

- **Inputs are only:**
  - the live schema (`\d` in `bird_<db>`),
  - `database_description/*.csv`,
  - the `bird.py evidence <db>` output.
- **Never** open the jsonl, the `.test.yml` files, gold SQL, `reports/`, or any `*.results.json` while authoring. An authoring agent is given only those three inputs.
- **Every evidence string becomes:**
  - a measure (a formula, e.g. "eligible free rate = free meal count / enrollment"),
  - a dimension (a coded value, e.g. "'POPLATEK TYDNE' stands for weekly issuance", as an `expr` or `description`),
  - or a description line on the member it concerns.
- **Coverage is recorded in `<db>/COVERAGE.md`**: one line per evidence string → the member that carries it.
- **Every table becomes a view**, with entities for its keys so joins resolve.
- **Every `datasource` is `bird_<db>`.**
- **The agent's context globs load only `*.view.yml` and `*.topic.yml`**, so `COVERAGE.md` never reaches the agent.
