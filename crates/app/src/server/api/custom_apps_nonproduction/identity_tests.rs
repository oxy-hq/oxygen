//! Who a database's credential is, and which mappings are refused.

use super::*;

/// Whether `a` and `b` resolve to the same host and user.
async fn same_credential(a: &Database, b: &Database, secrets: &SecretsManager) -> bool {
    identity(a, secrets).await == identity(b, secrets).await
}

fn db(yaml: &str) -> Database {
    serde_yaml::from_str(yaml).expect("a config.yml database entry")
}

fn secrets() -> SecretsManager {
    SecretsManager::from_environment().expect("env secrets")
}

#[tokio::test]
async fn a_copied_entry_is_the_same_credential() {
    let production = db("name: ch\ntype: clickhouse\nhost: https://ch.example.com/\nuser: app\n");
    let copy = db("name: ch_staging\ntype: clickhouse\nhost: ch.example.com\nuser: app\n");
    assert!(same_credential(&production, &copy, &secrets()).await);
}

#[tokio::test]
async fn another_host_or_user_is_another_credential() {
    let production = db("name: pg\ntype: postgres\nhost: db.example.com\nuser: app\n");
    let other_user = db("name: pg_s\ntype: postgres\nhost: db.example.com\nuser: app_staging\n");
    let other_host = db("name: pg_s\ntype: postgres\nhost: staging.example.com\nuser: app\n");
    let s = secrets();
    assert!(!same_credential(&production, &other_user, &s).await);
    assert!(!same_credential(&production, &other_host, &s).await);
}

/// Two entries reading one variable are one credential, resolved or not.
#[tokio::test]
async fn the_same_variable_is_the_same_credential() {
    let production =
        db("name: ch\ntype: clickhouse\nhost_var: OXY_TEST_UNSET_CH_HOST\nuser: app\n");
    let copy = db("name: ch_s\ntype: clickhouse\nhost_var: OXY_TEST_UNSET_CH_HOST\nuser: app\n");
    assert!(same_credential(&production, &copy, &secrets()).await);
}

#[tokio::test]
async fn an_unset_clickhouse_user_is_default() {
    let unset = db("name: ch\ntype: clickhouse\nhost: ch.example.com\n");
    let named = db("name: ch_s\ntype: clickhouse\nhost: ch.example.com\nuser: default\n");
    assert!(same_credential(&unset, &named, &secrets()).await);
}

#[tokio::test]
async fn a_mapping_onto_itself_or_the_workspaces_airhouse_is_refused() {
    let s = secrets();
    let ch = db("name: ch\ntype: clickhouse\nhost: ch.example.com\nuser: app\n");
    let ah = db("name: ah\ntype: airhouse_managed\n");
    let ah2 = db("name: ah2\ntype: airhouse_managed\n");
    let own = mapping_refusal(&ch, &ch, &s, Unresolved::CompareByName).await;
    assert!(own.unwrap().contains("onto itself"));
    for production in [&ch, &ah2] {
        let refused = mapping_refusal(production, &ah, &s, Unresolved::CompareByName).await;
        assert!(refused.unwrap().contains("airhouse_managed"));
    }
    let staging =
        db("name: ch_s\ntype: clickhouse\nhost: ch.example.com\nuser: app_s\ndatabase: staging\n");
    assert_eq!(
        mapping_refusal(&ch, &staging, &s, Unresolved::Refuse).await,
        None
    );
    // Both unset read ClickHouse's `default`: one database, refused.
    let same_default = db("name: ch_d\ntype: clickhouse\nhost: ch.example.com\nuser: app_s\n");
    assert!(
        mapping_refusal(&ch, &same_default, &s, Unresolved::Refuse)
            .await
            .unwrap()
            .contains("same database on the same host")
    );
}

/// An unresolved host is compared by name at publish, and refused at
/// invocation — the host fails closed.
#[tokio::test]
async fn an_unresolved_host_fails_closed_at_invocation_only() {
    let s = secrets();
    let ch = db("name: ch\ntype: clickhouse\nhost: ch.example.com\nuser: app\n");
    let unset =
        db("name: ch_s\ntype: clickhouse\nhost_var: OXY_TEST_UNSET_STAGING_HOST\nuser: app\n");
    assert_eq!(
        mapping_refusal(&ch, &unset, &s, Unresolved::CompareByName).await,
        None
    );
    let refused = mapping_refusal(&ch, &unset, &s, Unresolved::Refuse).await;
    assert!(refused.unwrap().contains("did not resolve"));
}

#[tokio::test]
async fn two_managed_airhouse_entries_are_one_credential() {
    let a = db("name: a\ntype: airhouse_managed\n");
    let b = db("name: b\ntype: airhouse_managed\n");
    assert!(same_credential(&a, &b, &secrets()).await);
}

/// Two MotherDuck entries on one token are one credential, whatever database
/// each names: the token reaches both (B2).
#[tokio::test]
async fn motherduck_entries_sharing_a_token_are_one_credential() {
    let prod = db("name: md_prod\ntype: motherduck\ntoken_var: MD_TOKEN\ndatabase: prod\n");
    let staging =
        db("name: md_staging\ntype: motherduck\ntoken_var: MD_TOKEN\ndatabase: staging\n");
    let other =
        db("name: md_other\ntype: motherduck\ntoken_var: MD_STAGING_TOKEN\ndatabase: staging\n");
    let s = secrets();
    assert!(same_credential(&prod, &staging, &s).await);
    let refused = mapping_refusal(&prod, &staging, &s, Unresolved::CompareByName).await;
    assert!(refused.unwrap().contains("same host and user"));
    assert!(!same_credential(&prod, &other, &s).await);
}

/// DuckDB to DuckDB is refused outright: one process, and ATTACH reaches
/// production's file (B2).
#[tokio::test]
async fn a_duckdb_to_duckdb_mapping_is_refused_whatever_the_files() {
    let prod = db("name: duck\ntype: duckdb\npath: prod.duckdb\n");
    let staging = db("name: duck_s\ntype: duckdb\npath: staging.duckdb\n");
    let refused = mapping_refusal(&prod, &staging, &secrets(), Unresolved::CompareByName).await;
    assert!(refused.unwrap().contains("both are DuckDB"));
}

/// Snowflake folds unquoted names to one case (S4).
#[tokio::test]
async fn snowflake_account_and_user_compare_without_case() {
    let prod = db(
        "name: sf\ntype: snowflake\naccount: ACME-XY12\nusername: LOADER\nwarehouse: w\n\
         database: PROD\npassword_var: SF_PW\n",
    );
    let staging = db(
        "name: sf_s\ntype: snowflake\naccount: acme-xy12\nusername: loader\nwarehouse: w\n\
         database: STAGING\npassword_var: SF_PW\n",
    );
    assert!(same_credential(&prod, &staging, &secrets()).await);
}

#[tokio::test]
async fn the_configured_database_is_read_from_each_kind() {
    let s = secrets();
    let pg = db("name: pg\ntype: postgres\nhost: h\nuser: u\ndatabase: Orders\n");
    assert_eq!(
        configured_database(&pg, &s).await,
        Ok(Some("Orders".into()))
    );
    let ch = db("name: ch\ntype: clickhouse\nhost: h\n");
    assert_eq!(
        configured_database(&ch, &s).await,
        Ok(Some("default".into()))
    );
    let duck = db("name: d\ntype: duckdb\npath: x.duckdb\n");
    assert_eq!(configured_database(&duck, &s).await, Ok(None));
    let unset =
        db("name: pg\ntype: postgres\nhost: h\nuser: u\ndatabase_var: OXY_TEST_P5B_UNSET_DB\n");
    assert!(configured_database(&unset, &s).await.is_err());
}

/// Fix round 2: the same database on the same host is production's, whatever
/// user signs in to it.
#[tokio::test]
async fn the_same_database_on_the_same_host_under_another_user_is_refused() {
    let s = secrets();
    let prod = db("name: pg\ntype: postgres\nhost: db.example.com\nuser: app\ndatabase: shop\n");
    let other_user =
        db("name: pg_s\ntype: postgres\nhost: db.example.com\nuser: app_staging\ndatabase: SHOP\n");
    let refused = mapping_refusal(&prod, &other_user, &s, Unresolved::CompareByName).await;
    assert!(refused.unwrap().contains("same database on the same host"));
    let other_database = db(
        "name: pg_s\ntype: postgres\nhost: db.example.com\nuser: app_staging\ndatabase: shop_staging\n",
    );
    assert_eq!(
        mapping_refusal(&prod, &other_database, &s, Unresolved::Refuse).await,
        None
    );
    let other_host =
        db("name: pg_s\ntype: postgres\nhost: staging.example.com\nuser: app\ndatabase: shop\n");
    assert_eq!(
        mapping_refusal(&prod, &other_host, &s, Unresolved::Refuse).await,
        None,
        "one name on two hosts is two databases"
    );
    let md_a = db("name: md_a\ntype: motherduck\ntoken_var: MD_A\ndatabase: shop\n");
    let md_b = db("name: md_b\ntype: motherduck\ntoken_var: MD_B\ndatabase: shop\n");
    let md = mapping_refusal(&md_a, &md_b, &s, Unresolved::CompareByName).await;
    assert!(md.unwrap().contains("same database on the same host"));
}

#[tokio::test]
async fn bigquery_names_its_datasets() {
    let bq =
        db("name: bq\ntype: bigquery\nkey_path: k.json\ndataset: sales\ndatasets:\n  events: []\n");
    let mut names = configured_names(&bq, &secrets()).await.unwrap();
    names.sort();
    assert_eq!(names, vec!["events".to_string(), "sales".to_string()]);
}
