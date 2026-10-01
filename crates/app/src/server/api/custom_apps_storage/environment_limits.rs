//! Storage limits and expiry for an app's **non-production** silos
//! (`customer-app-storage/<app_id>~<env>/`; environments design §4.2, §4.3).
//!
//! - **Metered, never counted against the org.** An environment's bytes are in
//!   its app's usage row and breakdown (under `~<env>/`), but the org quota
//!   reads production's bytes only ([`production_bytes`]), so a staging loop
//!   can never pause production's writes.
//! - **A cap of its own — a soft one.** Each environment silo is refused a
//!   write that would take it past `OXY_CUSTOMER_APPS_STORAGE_ENVIRONMENT_MAX_BYTES`
//!   (default 1 GiB). An invocation measures its silo once, on its first
//!   write, and adds what it writes after ([`SiloMeter`]), so it never lists
//!   the silo twice. The cap can be overshot: upload URLs are checked when
//!   minted, not when the browser PUTs, and invocations writing in parallel
//!   each count from their own measurement. That is acceptable because it
//!   bounds a runaway loop rather than rations: staging never counts toward the
//!   org quota, and the 30-day expiry clears whatever got past.
//! - **Thirty days, then gone.** An environment object older than 30 days is
//!   deleted by the storage sweeper ([`expired_environment_keys`]), matching
//!   the OLTP staging branch's 30-day horizon: production data copied into
//!   staging must not outlive production's own deletion commitment.

use uuid::Uuid;

use super::{Silo, StorageError, StoredObject, ops, quota::human_bytes, silo};

/// Default cap on one environment silo.
pub const DEFAULT_ENVIRONMENT_MAX_BYTES: u64 = 1024 * 1024 * 1024;

/// Age past which an environment object is swept.
pub const ENVIRONMENT_OBJECT_MAX_AGE_DAYS: i64 = 30;

/// Pages of 1,000 an invocation's measurement will walk. A silo with more
/// objects than that is refused rather than measured: it is staging.
const MAX_CAP_PAGES: usize = 50;

/// The per-silo cap in bytes (`OXY_CUSTOMER_APPS_STORAGE_ENVIRONMENT_MAX_BYTES`).
pub fn environment_max_bytes() -> u64 {
    std::env::var("OXY_CUSTOMER_APPS_STORAGE_ENVIRONMENT_MAX_BYTES")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(DEFAULT_ENVIRONMENT_MAX_BYTES)
}

/// The part of a usage row that counts toward the org quota: its total less
/// every `~<env>/…` bucket of its breakdown.
pub fn production_bytes(total: i64, breakdown: Option<&serde_json::Value>) -> i64 {
    let environments: i64 = breakdown
        .and_then(serde_json::Value::as_object)
        .map(|buckets| {
            buckets
                .iter()
                .filter(|(label, _)| label.starts_with('~'))
                .filter_map(|(_, usage)| usage.get("bytes")?.as_i64())
                .sum()
        })
        .unwrap_or(0);
    total.saturating_sub(environments).max(0)
}

/// Would `incoming` more bytes take a silo holding `used` past `cap`?
fn over_cap(used: u64, incoming: u64, cap: u64) -> bool {
    used.saturating_add(incoming) > cap
}

/// One invocation's running total for its environment silo: measured once, on
/// the first write the run asks to make, then advanced by each write it is
/// admitted — never re-listed. A **soft** cap (see the module docs): a
/// parallel invocation counts from its own measurement.
#[derive(Debug, Default)]
pub struct SiloMeter {
    /// `None` until the first write measures the silo.
    used: tokio::sync::Mutex<Option<u64>>,
}

impl SiloMeter {
    /// Admit a write of `incoming` bytes into an environment `silo`, or refuse
    /// it past the cap. Production's silo is not this meter's: it answers to
    /// the org quota (`quota::check_write_allowed`).
    pub async fn admit(&self, silo: &Silo, incoming: u64) -> Result<(), StorageError> {
        if silo.is_production() {
            return Ok(());
        }
        self.admit_measured(incoming, || silo_bytes(silo)).await
    }

    /// [`Self::admit`], measuring the silo with `measure` when this run has
    /// not yet.
    async fn admit_measured<F, Fut>(&self, incoming: u64, measure: F) -> Result<(), StorageError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<u64, StorageError>>,
    {
        let mut used = self.used.lock().await;
        let current = match *used {
            Some(bytes) => bytes,
            None => measure().await?,
        };
        *used = Some(current);
        let cap = environment_max_bytes();
        if over_cap(current, incoming, cap) {
            return Err(full(current, incoming, cap));
        }
        // Counted as new bytes even when the write overwrites a key: the
        // meter never under-counts.
        *used = Some(current.saturating_add(incoming));
        Ok(())
    }
}

/// The refusal a write past the cap answers.
fn full(used: u64, incoming: u64, cap: u64) -> StorageError {
    StorageError::TooLarge(format!(
        "this environment's storage is full: it holds {} of its {} cap, so this {} write \
         was refused. Delete assets from it (objects expire after {} days), or raise \
         OXY_CUSTOMER_APPS_STORAGE_ENVIRONMENT_MAX_BYTES. Production's storage is unaffected.",
        human_bytes(used as i64),
        human_bytes(cap as i64),
        human_bytes(incoming as i64),
        ENVIRONMENT_OBJECT_MAX_AGE_DAYS,
    ))
}

/// The bytes one silo holds now, walked live.
async fn silo_bytes(silo: &Silo) -> Result<u64, StorageError> {
    let prefix = silo.prefix();
    let (mut total, mut cursor) = (0u64, None);
    for _ in 0..MAX_CAP_PAGES {
        let page = ops::list_raw(&prefix, 1000, cursor).await?;
        total += page
            .objects
            .iter()
            .map(|o| o.size.max(0) as u64)
            .sum::<u64>();
        if !page.has_more || page.cursor.is_none() {
            return Ok(total);
        }
        cursor = page.cursor;
    }
    Err(StorageError::TooLarge(format!(
        "this environment's storage holds more objects than a write may count \
         ({MAX_CAP_PAGES}k); delete some before writing more"
    )))
}

/// The keys among `objects` that belong to one of `app_id`'s **environment**
/// silos and were last written more than [`ENVIRONMENT_OBJECT_MAX_AGE_DAYS`]
/// before `now`. Production's objects are never selected, whatever their age,
/// nor an object whose age cannot be read.
pub fn expired_environment_keys(
    app_id: Uuid,
    objects: &[StoredObject],
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<String> {
    let cutoff = now - chrono::Duration::days(ENVIRONMENT_OBJECT_MAX_AGE_DAYS);
    objects
        .iter()
        .filter(|o| matches!(silo::split_silo_key(app_id, &o.key), Some((Some(_), _))))
        .filter(|o| {
            o.last_modified
                .as_deref()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .is_some_and(|t| t < cutoff)
        })
        .map(|o| o.key.clone())
        .collect()
}

/// Delete every environment object of `app_id` older than
/// [`ENVIRONMENT_OBJECT_MAX_AGE_DAYS`]. Returns how many were deleted.
pub async fn sweep_expired(app_id: Uuid) -> Result<usize, StorageError> {
    let now = chrono::Utc::now();
    let mut deleted = 0;
    for root in super::environments::silo_roots(app_id).await? {
        if root == Silo::production(app_id).prefix() {
            continue;
        }
        let mut cursor = None;
        loop {
            let page = ops::list_raw(&root, 1000, cursor).await?;
            let expired = expired_environment_keys(app_id, &page.objects, now);
            deleted += ops::delete_raw(&expired).await?;
            if !page.has_more || page.cursor.is_none() {
                break;
            }
            cursor = page.cursor;
        }
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> Uuid {
        Uuid::from_u128(7)
    }

    fn object(key: String, age_days: i64) -> StoredObject {
        let at = chrono::Utc::now() - chrono::Duration::days(age_days);
        StoredObject {
            key,
            size: 1,
            content_type: None,
            last_modified: Some(at.to_rfc3339()),
        }
    }

    #[test]
    fn the_sweep_selects_only_environment_objects_past_thirty_days() {
        let a = app();
        let objects = vec![
            object(format!("customer-app-storage/{a}~staging/old.csv"), 31),
            object(format!("customer-app-storage/{a}~staging/new.csv"), 29),
            object(format!("customer-app-storage/{a}/old.csv"), 400),
            object(format!("customer-app-storage/{a}~dev-luong/x"), 45),
            object(
                format!("customer-app-storage/{}~staging/x", Uuid::from_u128(8)),
                90,
            ),
            StoredObject {
                last_modified: None,
                ..object(format!("customer-app-storage/{a}~staging/ageless"), 0)
            },
        ];
        assert_eq!(
            expired_environment_keys(a, &objects, chrono::Utc::now()),
            vec![
                format!("customer-app-storage/{a}~staging/old.csv"),
                format!("customer-app-storage/{a}~dev-luong/x"),
            ]
        );
    }

    #[test]
    fn production_bytes_leave_out_every_environment_bucket() {
        let breakdown = serde_json::json!({
            "uploads/": { "bytes": 100, "objects": 1 },
            "~staging/uploads/": { "bytes": 5000, "objects": 9 },
            "~staging/(root)": { "bytes": 20, "objects": 1 },
        });
        assert_eq!(production_bytes(5120, Some(&breakdown)), 100);
        assert_eq!(production_bytes(5120, None), 5120);
        assert_eq!(production_bytes(10, Some(&breakdown)), 0, "never negative");
    }

    /// A run lists its silo once: every later write is counted from that
    /// measurement plus what the run has written since.
    #[tokio::test]
    async fn a_second_write_in_one_invocation_does_not_re_list_the_silo() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        // SAFETY: nextest runs each test in its own process.
        unsafe { std::env::set_var("OXY_CUSTOMER_APPS_STORAGE_ENVIRONMENT_MAX_BYTES", "16") }
        let listings = AtomicUsize::new(0);
        let measure = || async {
            listings.fetch_add(1, Ordering::SeqCst);
            Ok(10)
        };
        let meter = SiloMeter::default();
        meter.admit_measured(4, measure).await.expect("10 + 4");
        meter.admit_measured(2, measure).await.expect("14 + 2 = 16");
        let refused = meter.admit_measured(1, measure).await.unwrap_err();
        assert!(matches!(refused, StorageError::TooLarge(_)), "{refused}");
        assert_eq!(listings.load(Ordering::SeqCst), 1, "measured once per run");
        meter
            .admit_measured(0, measure)
            .await
            .expect("a refused write added nothing");
    }

    #[test]
    fn a_write_past_the_cap_is_refused_and_one_up_to_it_is_not() {
        assert!(!over_cap(900, 100, 1000));
        assert!(over_cap(900, 101, 1000));
        assert!(over_cap(u64::MAX, 1, 1000));
    }
}
