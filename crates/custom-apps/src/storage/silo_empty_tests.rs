//! A silo without the fallback read: the storage plane's `empty` mode
//! (`internal-docs/2026-11-02-custom-app-branches-design.md` §3, plane 7).

use oxy_app_core::custom_app_environment::AppEnvironment;

use super::tests::use_temp_state_dir;
use super::*;

fn no_retention() -> RetentionPolicy {
    RetentionPolicy::default()
}

/// Write `pathname` into `silo` and return the key the store gave it.
async fn put_text(silo: &Silo, pathname: &str) -> String {
    put(
        silo,
        pathname,
        b"text".to_vec(),
        PutOptions::default(),
        &no_retention(),
    )
    .await
    .expect("put")
    .key
}

/// A key that exists only in production is found through the fallback, and
/// reads as absent once the silo has none.
#[tokio::test]
async fn a_silo_without_fallback_never_reads_productions_key() {
    let tmp = use_temp_state_dir();
    let app = Uuid::new_v4();
    let production_key = put_text(&Silo::production(app), "uploads/only-prod.txt").await;

    let branch = Silo::for_environment(app, &AppEnvironment::Staging);
    assert!(get(&branch, &production_key).await.unwrap().is_some());
    assert!(head(&branch, &production_key).await.unwrap().is_some());

    let empty = branch.clone().without_fallback();
    assert!(get(&empty, &production_key).await.unwrap().is_none());
    assert!(head(&empty, &production_key).await.unwrap().is_none());
    // Production still has it: nothing was deleted or shadowed.
    assert!(
        get(&Silo::production(app), &production_key)
            .await
            .unwrap()
            .is_some()
    );
    let _ = std::fs::remove_dir_all(tmp);
}

/// The silo's own objects are still read, and its listing and writes are
/// unchanged: only the fallback is gone.
#[tokio::test]
async fn a_silo_without_fallback_still_reads_its_own_objects() {
    let tmp = use_temp_state_dir();
    let app = Uuid::new_v4();
    let empty = Silo::for_environment(app, &AppEnvironment::Staging).without_fallback();
    let own = put_text(&empty, "uploads/own.txt").await;
    assert!(get(&empty, &own).await.unwrap().is_some());
    let page = list(&empty, None, None, None).await.unwrap();
    assert_eq!(page.objects.len(), 1);
    let _ = std::fs::remove_dir_all(tmp);
}

#[test]
fn without_fallback_changes_nothing_but_the_fallback() {
    let app = Uuid::new_v4();
    let branch = Silo::for_environment(app, &AppEnvironment::Staging);
    let empty = branch.clone().without_fallback();
    assert_eq!(empty.prefix(), branch.prefix());
    assert_eq!(empty.fallback(), None);
    assert_eq!(branch.fallback(), Some(Silo::production(app)));
    assert_ne!(empty, branch);
    // A production key is still re-rooted into the silo for a write.
    let production_key = format!("customer-app-storage/{app}/uploads/a.pdf");
    assert_eq!(empty.relative(&production_key), Some("uploads/a.pdf"));
    // Production has none to drop.
    assert_eq!(
        Silo::production(app).without_fallback(),
        Silo::production(app)
    );
}
