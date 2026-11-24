//! A branch cut from production after an app opted in to capture (OLTP change
//! stream design §8 phase 1): every cut and reset runs
//! [`SEED_BRANCH_STREAM_SQL`] as the branch's owner, and from then the
//! branch's writes succeed and write no log row.
//!
//! Run against a plain store with an installed table, which is what a cut
//! copies: the statement is what is under test, not the provider's copy.
//! Which cuts run it is the provisioner's (`provisioner/branch_changes.rs`).

use oxy_oltp::changes::SEED_BRANCH_STREAM_SQL;
use oxy_oltp::platform::CHANGES_TABLES_TABLE;

use crate::changes_capture::Store;

struct Seen {
    inherited: usize,
    writes: Result<(), String>,
    logged: usize,
    stream: Vec<(i64, bool)>,
    installed: i64,
    again: Result<(), String>,
}

async fn cut(store: &Store) -> Result<Seen, String> {
    store.leases(&[]).await?;
    store
        .as_writer("INSERT INTO leases VALUES (1, 'a', 'a@x'), (2, 'b', 'b@x')")
        .await?;
    let inherited = store.log().await?.len();
    store.as_owner(SEED_BRANCH_STREAM_SQL).await?;
    let writes = store
        .as_writer(
            "INSERT INTO leases VALUES (3, 'c', 'c@x'); \
             UPDATE leases SET name = 'z'; \
             DELETE FROM leases WHERE id = 1; \
             TRUNCATE leases",
        )
        .await;
    let logged = store.log().await?.len();
    let installed = store
        .owner_count(&format!(
            "SELECT count(*) FROM {CHANGES_TABLES_TABLE} WHERE installed"
        ))
        .await?;
    // A reset seeds a branch a cut already seeded.
    let again = store.as_owner(SEED_BRANCH_STREAM_SQL).await;
    Ok(Seen {
        inherited,
        writes,
        logged,
        stream: store.stream().await?,
        installed,
        again,
    })
}

#[tokio::test]
async fn a_cut_branch_does_not_capture() {
    let Some(store) = Store::create("cap_branch").await else {
        return;
    };
    let out = cut(&store).await;
    store.cleanup().await;

    let seen = out.expect("install, write, seed the stream and write again");
    assert_eq!(
        seen.inherited, 2,
        "the log rows a copy of production carries"
    );
    seen.writes.expect("the branch's writes succeed");
    assert_eq!(
        seen.logged, 0,
        "the inherited rows are gone, and no write added one"
    );
    assert_eq!(
        seen.stream,
        [(0, false)],
        "one stream row, at generation 0, not capturing"
    );
    assert_eq!(
        seen.installed, 1,
        "production's capture stays installed, so the DDL guard holds on the branch"
    );
    seen.again.expect("seeding a seeded branch succeeds");
}
