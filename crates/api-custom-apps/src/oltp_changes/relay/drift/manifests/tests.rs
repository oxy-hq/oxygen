//! What production's builds declare, read once a build: a second pass on the
//! same build reads no manifest, and a promote's new build is read by the
//! next pass.

use std::collections::HashMap;
use std::future::{Ready, ready};
use std::sync::{Mutex, PoisonError};

use oxy_custom_apps::manifest::OltpChangesConfig;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{Declaration, Manifests, ReadBuilds};

/// A block declaring each of `tables` under `oltpChanges`.
fn declaring(tables: &[&str]) -> OltpChangesConfig {
    let tables: serde_json::Map<String, Value> = tables
        .iter()
        .map(|table| ((*table).to_owned(), json!({})))
        .collect();
    serde_json::from_value(json!({ "tables": tables })).expect("an oltpChanges block")
}

/// The platform's builds, and every read the drift check asked of them.
#[derive(Default)]
struct Builds {
    declared: HashMap<Uuid, Option<OltpChangesConfig>>,
    reads: Mutex<Vec<Vec<Uuid>>>,
}

impl Builds {
    /// A new build declaring `declared`.
    fn publish(&mut self, declared: Option<OltpChangesConfig>) -> Uuid {
        let build = Uuid::new_v4();
        self.declared.insert(build, declared);
        build
    }

    /// One pass's declarations, production at `live`'s `(app, build)`s.
    async fn pass(
        &self,
        manifests: &Manifests,
        live: &[(Uuid, Uuid)],
    ) -> HashMap<Uuid, Declaration> {
        manifests
            .declared(live, |builds| self.read(builds))
            .await
            .expect("the pass's read")
    }

    /// The `app_builds` query: what each of `builds` it finds declares.
    fn read(&self, mut builds: Vec<Uuid>) -> Ready<ReadBuilds> {
        builds.sort();
        let found = builds
            .iter()
            .filter_map(|build| Some((*build, self.declared.get(build)?.clone())))
            .collect();
        self.reads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(builds);
        ready(Ok(found))
    }

    /// The reads since the last call, each one's builds sorted.
    fn reads(&self) -> Vec<Vec<Uuid>> {
        std::mem::take(&mut *self.reads.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// The tables `app` declares in a pass's `declared`, none for a build that
/// declares nothing; the app must be there.
fn tables(declared: &HashMap<Uuid, Declaration>, app: Uuid) -> Vec<String> {
    let declaration = declared.get(&app).expect("the app is declared");
    declaration
        .as_ref()
        .map(|config| config.tables.keys().cloned().collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn a_second_pass_on_the_same_build_reads_no_manifest_and_a_promote_reads_its_build() {
    let mut builds = Builds::default();
    let app = Uuid::new_v4();
    let first = builds.publish(Some(declaring(&["leases"])));
    let second = builds.publish(Some(declaring(&["leases", "notes"])));
    let manifests = Manifests::new(16);

    let declared = builds.pass(&manifests, &[(app, first)]).await;
    assert_eq!(builds.reads(), [vec![first]], "the first pass reads it");
    assert_eq!(tables(&declared, app), ["leases"]);

    let declared = builds.pass(&manifests, &[(app, first)]).await;
    assert!(builds.reads().is_empty(), "the second reads no manifest");
    assert_eq!(tables(&declared, app), ["leases"]);

    // A promote moves production's pointer to the second build.
    let declared = builds.pass(&manifests, &[(app, second)]).await;
    assert_eq!(builds.reads(), [vec![second]], "the next pass reads it");
    assert_eq!(tables(&declared, app), ["leases", "notes"]);

    builds.pass(&manifests, &[(app, second)]).await;
    assert!(builds.reads().is_empty(), "and the one after reads none");

    // A rollback moves it back: an app keeps one build, so it is read again.
    let declared = builds.pass(&manifests, &[(app, first)]).await;
    assert_eq!(builds.reads(), [vec![first]]);
    assert_eq!(tables(&declared, app), ["leases"]);
}

#[tokio::test]
async fn a_pass_reads_only_the_builds_that_changed() {
    let mut builds = Builds::default();
    let (kept, promoted) = (Uuid::new_v4(), Uuid::new_v4());
    let kept_build = builds.publish(Some(declaring(&["leases"])));
    let before = builds.publish(Some(declaring(&["units"])));
    let after = builds.publish(Some(declaring(&["units", "tenants"])));
    let manifests = Manifests::new(16);

    builds
        .pass(&manifests, &[(kept, kept_build), (promoted, before)])
        .await;
    let mut both = vec![kept_build, before];
    both.sort();
    assert_eq!(builds.reads(), [both], "one read for every app at first");

    let declared = builds
        .pass(&manifests, &[(kept, kept_build), (promoted, after)])
        .await;
    assert_eq!(
        builds.reads(),
        [vec![after]],
        "only the promoted app's build"
    );
    assert_eq!(tables(&declared, kept), ["leases"]);
    assert_eq!(tables(&declared, promoted), ["tenants", "units"]);
}

#[tokio::test]
async fn a_build_declaring_nothing_is_kept_too() {
    let mut builds = Builds::default();
    let app = Uuid::new_v4();
    let build = builds.publish(None);
    let manifests = Manifests::new(16);

    builds.pass(&manifests, &[(app, build)]).await;
    assert_eq!(builds.reads(), [vec![build]]);
    let declared = builds.pass(&manifests, &[(app, build)]).await;
    assert!(builds.reads().is_empty(), "not read again");
    assert_eq!(tables(&declared, app), Vec::<String>::new());
}

#[tokio::test]
async fn a_build_the_read_does_not_find_is_asked_for_again() {
    let builds = Builds::default();
    let (app, missing) = (Uuid::new_v4(), Uuid::new_v4());
    let manifests = Manifests::new(16);

    let declared = builds.pass(&manifests, &[(app, missing)]).await;
    assert!(!declared.contains_key(&app), "left out");
    builds.pass(&manifests, &[(app, missing)]).await;
    assert_eq!(builds.reads(), [vec![missing], vec![missing]], "not kept");
}

#[tokio::test]
async fn past_its_bound_the_least_recently_checked_app_is_dropped() {
    let mut builds = Builds::default();
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let (a_build, b_build) = (builds.publish(None), builds.publish(None));
    let manifests = Manifests::new(1);

    builds.pass(&manifests, &[(a, a_build)]).await;
    builds.pass(&manifests, &[(b, b_build)]).await;
    builds.pass(&manifests, &[(b, b_build)]).await;
    builds.pass(&manifests, &[(a, a_build)]).await;
    assert_eq!(
        builds.reads(),
        [vec![a_build], vec![b_build], vec![a_build]],
        "b is kept, and a, dropped for it, is read again"
    );
}
