//! The publish pipeline's stages by name, and the timer that clocks them.
//!
//! [`Stage`] is the one list of the pipeline's stages: the timer keys its
//! clock by it, the attempt row stores its timings under [`Stage::name`], and
//! the pipeline's own header names them in the same order (a test holds the
//! two together). A timing therefore cannot name a stage that does not exist,
//! nor a stage be renamed in one place and not the other.

use std::future::Future;
use std::time::{Duration, Instant};

use serde::Serialize;

/// The publish pipeline's stages, in the order they run
/// (`custom_apps_publish::pipeline`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Resolve the org, authorize, refuse what can be refused. Writes nothing.
    Admit,
    /// Decompress and check the bundle. Writes nothing.
    Inflate,
    /// Upsert the app row, then put the bundle's bytes in the store: the
    /// first stage that writes, so the app row's upsert is clocked here.
    Store,
    /// The build row and its functions.
    Record,
    /// A promoting publish's schema migrations.
    Promote,
    /// Move the pointers and reconcile their schedules.
    Point,
    /// What follows the move and cannot fail the publish: GC, caches, queued
    /// migrations.
    After,
}

impl Stage {
    /// Every stage, in pipeline order.
    pub const ALL: [Stage; 7] = [
        Stage::Admit,
        Stage::Inflate,
        Stage::Store,
        Stage::Record,
        Stage::Promote,
        Stage::Point,
        Stage::After,
    ];

    /// The name a timing and a failed attempt record the stage under.
    pub const fn name(self) -> &'static str {
        match self {
            Stage::Admit => "admit",
            Stage::Inflate => "inflate",
            Stage::Store => "store",
            Stage::Record => "record",
            Stage::Promote => "promote",
            Stage::Point => "point",
            Stage::After => "after",
        }
    }

    /// The stage named `name`, if this binary knows it.
    pub fn from_name(name: &str) -> Option<Stage> {
        Self::ALL.into_iter().find(|stage| stage.name() == name)
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// One stage's wall time, as a publish answers it and the console reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StageTiming {
    pub stage: String,
    pub ms: u64,
}

/// Clocks each stage of one publish as it runs, and remembers the one that
/// failed.
#[derive(Debug, Default)]
pub struct StageTimer {
    ms: [Option<u64>; Stage::ALL.len()],
    failed: Option<Stage>,
}

impl StageTimer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `work` as `stage`. Its time adds to whatever the stage already
    /// clocked (a stage may run in more than one piece), and an `Err` marks
    /// `stage` the one the publish failed at — the first such, if two do.
    pub async fn run<T, E>(
        &mut self,
        stage: Stage,
        work: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        let started = Instant::now();
        let outcome = work.await;
        self.clock(stage, started.elapsed());
        if outcome.is_err() {
            self.fail(stage);
        }
        outcome
    }

    /// Run `work`, which cannot fail, as `stage`.
    pub async fn run_infallible<T>(&mut self, stage: Stage, work: impl Future<Output = T>) -> T {
        let started = Instant::now();
        let out = work.await;
        self.clock(stage, started.elapsed());
        out
    }

    /// Add `elapsed` to `stage`'s time.
    pub fn clock(&mut self, stage: Stage, elapsed: Duration) {
        let slot = &mut self.ms[stage.index()];
        *slot = Some(slot.unwrap_or(0).saturating_add(millis(elapsed)));
    }

    /// Mark `stage` the one the publish failed at, unless one already is.
    pub fn fail(&mut self, stage: Stage) {
        self.failed.get_or_insert(stage);
    }

    /// The stage the publish failed at, if it failed in one.
    pub fn failed(&self) -> Option<Stage> {
        self.failed
    }

    /// Every stage that ran, in pipeline order.
    pub fn timings(&self) -> Vec<StageTiming> {
        Stage::ALL
            .into_iter()
            .filter_map(|stage| {
                self.ms[stage.index()].map(|ms| StageTiming {
                    stage: stage.name().to_string(),
                    ms,
                })
            })
            .collect()
    }

    /// `{stage: ms}` for every stage that ran: what the attempt row stores.
    pub fn to_json(&self) -> serde_json::Value {
        let map = self
            .timings()
            .into_iter()
            .map(|t| (t.stage, serde_json::Value::from(t.ms)))
            .collect::<serde_json::Map<_, _>>();
        serde_json::Value::Object(map)
    }
}

/// A stored `{stage: ms}` read back in pipeline order. JSONB keeps no key
/// order, so the order is [`Stage::ALL`]'s; a key this binary does not know
/// (a stage added by a newer release) follows, by name, so nothing a row holds
/// is dropped. A value that is not a non-negative integer is skipped.
pub fn timings_from_json(value: &serde_json::Value) -> Vec<StageTiming> {
    let Some(map) = value.as_object() else {
        return Vec::new();
    };
    let ms_of = |key: &str| map.get(key).and_then(serde_json::Value::as_u64);
    let known = Stage::ALL.into_iter().filter_map(|stage| {
        ms_of(stage.name()).map(|ms| StageTiming {
            stage: stage.name().to_string(),
            ms,
        })
    });
    let mut unknown: Vec<StageTiming> = map
        .keys()
        .filter(|key| Stage::from_name(key).is_none())
        .filter_map(|key| {
            ms_of(key).map(|ms| StageTiming {
                stage: key.clone(),
                ms,
            })
        })
        .collect();
    unknown.sort_by(|a, b| a.stage.cmp(&b.stage));
    known.chain(unknown).collect()
}

fn millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[path = "stages_tests.rs"]
mod tests;
