//! The restore check (OLTP change stream design §5.5, "A restore is found by
//! an exact check, not a threshold"): before a writer's batch is read, the
//! last batch its Airhouse `_progress` holds is compared with the store's
//! `oxy_changes.progress` row for it. Exact, so it needs no threshold, and the
//! same whether the platform ran the restore or someone else did.

/// The last batch Airhouse holds for a writer: its `_progress` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shipped {
    pub generation: i64,
    pub batch: i64,
}

impl Shipped {
    /// A row of `render::progress_read_sql`, every value as text.
    pub fn from_row(row: &[Option<String>]) -> Result<Shipped, String> {
        let cell = |i: usize, name: &str| -> Result<i64, String> {
            let text = row
                .get(i)
                .and_then(Option::as_deref)
                .ok_or_else(|| format!("_progress.{name} is missing"))?;
            text.parse()
                .map_err(|e| format!("_progress.{name} {text:?}: {e}"))
        };
        Ok(Shipped {
            generation: cell(0, "generation")?,
            batch: cell(1, "batch")?,
        })
    }
}

/// The store's side: its stream's generation, and the writer's
/// `intent_batch` and `shipped_batch` (both 0 before its first batch).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreProgress {
    pub generation: i64,
    pub intent: i64,
    pub shipped: i64,
}

/// What the check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing was lost: ship the next batch.
    Proceed,
    /// Airhouse is past the store: the store was restored to before a batch
    /// Airhouse holds, so new changes could reuse its keys.
    Rewound,
    /// Airhouse is behind the store: it lost batches the log no longer holds.
    Lost,
}

/// The check, rule by rule; the first that applies decides.
///
/// | Airhouse | Verdict |
/// | --- | --- |
/// | nothing, and the store shipped a batch | `Lost` |
/// | nothing | `Proceed` |
/// | past `intent`, or a later generation | `Rewound` |
/// | an earlier generation | `Proceed`: the store bumped since |
/// | behind `shipped` | `Lost` |
/// | otherwise | `Proceed` |
///
/// The last row covers Airhouse at `shipped` (caught up) and between
/// `shipped` and `intent`: a relay that died after the Airhouse commit and
/// before it confirmed, whose batch the next one ships again and the
/// anti-join skips.
pub fn check(airhouse: Option<Shipped>, store: StoreProgress) -> Verdict {
    let Some(airhouse) = airhouse else {
        return if store.shipped > 0 {
            Verdict::Lost
        } else {
            Verdict::Proceed
        };
    };
    if airhouse.batch > store.intent || airhouse.generation > store.generation {
        return Verdict::Rewound;
    }
    if airhouse.generation < store.generation {
        return Verdict::Proceed;
    }
    if airhouse.batch < store.shipped {
        return Verdict::Lost;
    }
    Verdict::Proceed
}

#[cfg(test)]
mod tests;
