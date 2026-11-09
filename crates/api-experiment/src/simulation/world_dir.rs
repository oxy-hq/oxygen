//! The per-run materialised workspace.
//!
//! A run writes `config.yml`, a `.view.yml` and a dataset directory into a
//! `TempDir` and points the loader at it — the materialiser pattern
//! `semantic_scan.rs` already uses for airlayer, applied to a whole tiny
//! workspace rather than one file.
//!
//! Why not register a database on the real workspace: that mutates workspace
//! config per run and needs cleanup that survives a crash. Here the `TempDir`
//! *is* the cleanup, and a run that dies mid-flight leaks nothing. The
//! artifacts are still real files read by the real loader, so the run still
//! exercises the path a customer's data takes — which is the whole reason a
//! result here says anything about the shipped product.
//!
//! # The storage is a dataset directory, not a `.duckdb` file
//!
//! Oxy's local DuckDB mode maps one file in the directory to one table. Rows
//! are appended to `store_days.csv`, and `PoolKey::local` evicts the cached
//! in-memory database when the file's mtime changes — which is what Phase 0
//! verified end to end. One hazard worth naming: that key carries no file
//! size, so on a filesystem with whole-second mtime resolution two appends
//! inside one tick would collide and the fitter would silently read a frozen
//! world. APFS and ext4 both report nanoseconds.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use oxy_shared::errors::OxyError;
use oxy_simulation::{EntityDay, RowSink, SimulationError, SimulationSpec};
use tempfile::TempDir;

/// Datasource name the generated `config.yml` and `.view.yml` agree on.
pub const DATASOURCE: &str = "sim";
/// The one table a Phase 1 world emits. File stem = table name.
pub const TABLE: &str = "store_days";

/// A workspace that exists only for the duration of one run.
pub struct WorldDir {
    dir: TempDir,
}

impl WorldDir {
    /// Write `config.yml`, the view and an empty dataset directory.
    pub fn create(spec: &SimulationSpec) -> Result<Self, OxyError> {
        let dir = TempDir::new()
            .map_err(|e| OxyError::RuntimeError(format!("simulation workspace: {e}")))?;
        let root = dir.path();
        let data = root.join("data");
        fs::create_dir_all(root.join("semantics/views"))
            .and_then(|_| fs::create_dir_all(&data))
            .map_err(|e| OxyError::RuntimeError(format!("simulation workspace: {e}")))?;

        write(&root.join("config.yml"), &config_yml(&data))?;
        write(
            &root
                .join("semantics/views")
                .join(format!("{TABLE}.view.yml")),
            &view_yml(spec),
        )?;
        // The header has to exist before the first append or DuckDB infers the
        // schema from whatever the first data row happens to look like.
        write(&data.join(format!("{TABLE}.csv")), &csv_header(spec))?;
        Ok(Self { dir })
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn dataset_dir(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    pub fn table_csv(&self) -> PathBuf {
        self.dataset_dir().join(format!("{TABLE}.csv"))
    }
}

/// Hand the dataset directory back to the DuckDB pool.
///
/// The pool caches one in-memory DuckDB per dataset directory and evicts it
/// only by same-key replacement — a key that recurs when the directory is a
/// workspace's, and never when it is a run's. So each run's probe leaves a live
/// in-memory database, holding a full copy of `store_days`, keyed on a `TempDir`
/// that is about to be deleted. Nothing would ever check it out again, and
/// nothing would ever drop it.
///
/// Here rather than at the call site in `mod.rs` because this is the type that
/// owns the directory: a run that returns early on a `?` or panics mid-period
/// still unwinds through this drop, and those are the paths a hand-written
/// release would miss.
///
/// Ordering matters and it is the ordering Rust gives us: this body runs
/// **before** the `dir` field is dropped, so the directory still exists on disk
/// and `release_local_connection` can canonicalize it into the same key the
/// checkout used. Releasing after the `TempDir` is gone would leave the path
/// uncanonicalizable — and on macOS, where every temp dir sits behind the
/// `/var` → `/private/var` symlink, the raw path is never the key, so the
/// release would silently do nothing.
impl Drop for WorldDir {
    fn drop(&mut self) {
        oxy::connector::release_local_connection(self.dataset_dir());
    }
}

/// The dataset CSV's header, in the column order [`CsvSink::append`] writes
/// rows in. `net_sales`/`marketing_spend` are `EntityDay`'s own field names —
/// internal Rust identifiers, generic to any driver-lifts-target mechanism —
/// so the header carries what this *world* calls them instead:
/// `spec.mechanism.target`/`.driver`. `SimulationSpec::validate` guarantees
/// neither collides with `entity_id`, `date`, `prime_cost`, or each other.
fn csv_header(spec: &SimulationSpec) -> String {
    format!(
        "entity_id,date,{},{},prime_cost\n",
        spec.mechanism.target, spec.mechanism.driver
    )
}

/// Appends a period's rows to the dataset CSV.
pub struct CsvSink {
    path: PathBuf,
}

impl CsvSink {
    pub fn new(world: &WorldDir) -> Self {
        Self {
            path: world.table_csv(),
        }
    }
}

impl RowSink for CsvSink {
    fn append(&mut self, rows: &[EntityDay]) -> Result<(), SimulationError> {
        let mut body = String::with_capacity(rows.len() * 48);
        for row in rows {
            body.push_str(&format!(
                "{},{},{:.6},{:.6},{:.6}\n",
                row.entity_id, row.date, row.net_sales, row.marketing_spend, row.prime_cost
            ));
        }
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&self.path)
            .map_err(|e| SimulationError::Write(format!("open {}: {e}", self.path.display())))?;
        file.write_all(body.as_bytes())
            .map_err(|e| SimulationError::Write(format!("append rows: {e}")))?;
        // Without this the fitter can read a file whose tail is still in the
        // OS buffer — a short history, silently, and a refusal that reads as
        // "unidentified" rather than "not written yet".
        file.flush()
            .map_err(|e| SimulationError::Write(format!("flush rows: {e}")))
    }
}

fn write(path: &Path, contents: &str) -> Result<(), OxyError> {
    fs::write(path, contents)
        .map_err(|e| OxyError::RuntimeError(format!("write {}: {e}", path.display())))
}

fn config_yml(dataset_dir: &Path) -> String {
    format!(
        "databases:\n  - name: {DATASOURCE}\n    type: duckdb\n    dataset: {}\n",
        dataset_dir.display()
    )
}

/// The semantic layer for the simulated world.
///
/// The driver edge carries **no coefficient** on purpose — that is exactly what
/// makes it fittable (`fittable_edges` skips declared coefficients), and fitting
/// it is the thing under test.
///
/// `lag:` comes from `declared_lag`, **not** from `lag_days`. It is declared by
/// a human and never fitted, so on real data it is a guess — and reading the
/// world's true lag here would make the customer right by construction and put
/// the whole lag-error axis out of reach. A world that says nothing about it
/// gets the true lag, which is the case where the guess happened to be right.
///
/// The driver/target measures' `name:` and `expr:` are the same string —
/// `spec.mechanism.driver`/`.target` — and that is safe precisely because
/// [`csv_header`] writes the dataset CSV under those same names: the column
/// this `expr:` binds to is guaranteed to exist, whatever the world calls it.
fn view_yml(spec: &SimulationSpec) -> String {
    let m = &spec.mechanism;
    format!(
        r#"name: {TABLE}
description: |
  A declared world, generated by oxy-simulation. Every row here was produced by
  a mechanism whose true parameters are recorded on the run — which is the one
  thing no customer workspace can offer.
datasource: {DATASOURCE}
table: {TABLE}

entities:
  # The row's own grain. `store` stays foreign because that is what makes it a
  # panel identifier — `fit_panel_dimensions` reads foreign entities, and the
  # within-panel demeaning is the whole point of the fit. The loader still warns
  # that `store` has no matching primary view; harmless with one view and no
  # joins, and the alternative (a stub store view) would add a join to a world
  # that has nothing to join to.
  - name: store_day
    type: primary
    description: "One entity on one business date"
    keys: [entity_id, date]
  - name: store
    type: foreign
    description: "The panel the fit demeans within"
    key: entity_id

dimensions:
  - name: entity_id
    type: number
    expr: entity_id
  - name: date
    type: date
    expr: date

measures:
  - name: {driver}
    type: sum
    expr: {driver}
  - name: prime_cost
    type: sum
    expr: prime_cost
  - name: {target}
    type: sum
    expr: {target}
    drivers:
      - measure: {TABLE}.{driver}
        direction: positive
        strength: strong
        confidence: high
        lag: {lag}
        description: "Declared as a claim, sized from history. The claim under test."
"#,
        driver = m.driver,
        target = m.target,
        lag = m.declared_lag(),
    )
}

#[cfg(test)]
mod tests;
