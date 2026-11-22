//! Connector translation: which `*_var` failures are errors and which unset
//! fields keep their defaults. Environment-backed managers, no database; the
//! oxydb-backed half is `tests/platform/connector_vars_from_workspace_secrets.rs`.

/// A manager over a `config.yml` holding `databases_yaml`, resolving
/// secrets from the process environment only.
async fn env_manager(
    databases_yaml: &str,
) -> (
    oxy_test_utils::fixtures::TestFixture,
    oxy::adapters::workspace::manager::WorkspaceManager<oxy::config::WorkingCopy>,
) {
    use oxy::adapters::workspace::builder::WorkspaceBuilder;
    let fixture = oxy_test_utils::fixtures::TestFixture::new().expect("tempdir");
    fixture
        .create_file(
            "config.yml",
            &format!("models: []\ndatabases:\n{databases_yaml}"),
        )
        .expect("write config.yml");
    let wm = WorkspaceBuilder::new(uuid::Uuid::new_v4())
        .with_working_copy(fixture.path(), None, oxy::config::OnMissing::Fail)
        .await
        .expect("config builder")
        .with_secrets_manager(oxy::adapters::secrets::SecretsManager::from_environment().unwrap())
        .build()
        .await
        .expect("workspace manager");
    (fixture, wm)
}

async fn translate(
    wm: &oxy::adapters::workspace::manager::WorkspaceManager<oxy::config::WorkingCopy>,
    name: &str,
) -> Result<Option<agentic_connector::ConnectorConfig>, oxy_shared::errors::OxyError> {
    let db = wm
        .config_manager
        .resolve_database(name)
        .expect("configured");
    super::try_database_to_connector_config(&db, wm).await
}

/// The Poke House failure: a ClickHouse `host_var` that names no secret
/// became `http://localhost:8123` with `database = ''`. It is an error that
/// names the secret, and the `Option` API used by batch resolution returns
/// `None` rather than a connector to the wrong host.
#[tokio::test]
async fn a_clickhouse_host_var_naming_no_secret_is_an_error_not_localhost() {
    let (_fx, wm) = env_manager(
        "  - name: clickhouse\n    type: clickhouse\n    host_var: S10_UNIT_CH_HOST_UNSET\n",
    )
    .await;
    let err = translate(&wm, "clickhouse")
        .await
        .expect_err("an unresolved host_var is an error");
    let message = err.to_string();
    assert!(
        message.contains("S10_UNIT_CH_HOST_UNSET") && message.contains("host_var"),
        "names the var: {message}"
    );
    let db = wm.config_manager.resolve_database("clickhouse").unwrap();
    assert!(
        super::database_to_connector_config(&db, &wm)
            .await
            .is_none()
    );
}

/// The same var, once it resolves, is the URL — scheme and port kept.
#[tokio::test]
async fn a_clickhouse_host_var_that_resolves_is_the_url() {
    // SAFETY: nextest runs each test in its own process.
    unsafe {
        std::env::set_var("S10_UNIT_CH_HOST", "https://ch.example.test:8443");
        std::env::set_var("S10_UNIT_CH_DATABASE", "restaurant_analytics");
    }
    let (_fx, wm) = env_manager(
        "  - name: clickhouse\n    type: clickhouse\n    host_var: S10_UNIT_CH_HOST\n    \
         database_var: S10_UNIT_CH_DATABASE\n",
    )
    .await;
    let Some(agentic_connector::ConnectorConfig::ClickHouse(ch)) =
        translate(&wm, "clickhouse").await.expect("resolves")
    else {
        panic!("expected a ClickHouse config");
    };
    assert_eq!(ch.url, "https://ch.example.test:8443");
    assert_eq!(ch.database, "restaurant_analytics");
    assert_eq!(ch.user, "", "an unset user is the server's default");
}

/// Back-compat: a host the author never configured keeps `localhost` — the
/// developer's own ClickHouse. Only a NAMED var that does not resolve fails.
#[tokio::test]
async fn an_unset_clickhouse_host_keeps_the_localhost_default() {
    let (_fx, wm) = env_manager("  - name: clickhouse\n    type: clickhouse\n").await;
    let Some(agentic_connector::ConnectorConfig::ClickHouse(ch)) =
        translate(&wm, "clickhouse").await.expect("resolves")
    else {
        panic!("expected a ClickHouse config");
    };
    assert_eq!(ch.url, "http://localhost:8123");
}

/// Postgres, Redshift and MySQL defaulted every unresolved field without a
/// word — `host_var` to `localhost` — so they fail the same way.
#[tokio::test]
async fn postgres_redshift_and_mysql_vars_naming_no_secret_are_errors() {
    let (_fx, wm) = env_manager(
        "  - name: pg\n    type: postgres\n    host_var: S10_UNIT_PG_HOST_UNSET\n\
         \x20 - name: rs\n    type: redshift\n    password_var: S10_UNIT_RS_PASSWORD_UNSET\n\
         \x20 - name: my\n    type: mysql\n    database_var: S10_UNIT_MY_DATABASE_UNSET\n",
    )
    .await;
    for (name, var, field) in [
        ("pg", "S10_UNIT_PG_HOST_UNSET", "host_var"),
        ("rs", "S10_UNIT_RS_PASSWORD_UNSET", "password_var"),
        ("my", "S10_UNIT_MY_DATABASE_UNSET", "database_var"),
    ] {
        let message = translate(&wm, name)
            .await
            .expect_err("an unresolved var is an error")
            .to_string();
        assert!(
            message.contains(var) && message.contains(field) && message.contains(name),
            "{name}: names the database, field and var: {message}"
        );
    }
}

/// A port var that resolves to something that is not a port used to become
/// the default port silently.
#[tokio::test]
async fn a_port_var_that_is_not_a_number_is_an_error() {
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("S10_UNIT_PG_PORT", "five-four-three-two") };
    let (_fx, wm) = env_manager(
        "  - name: pg\n    type: postgres\n    host: db.example.test\n    \
         port_var: S10_UNIT_PG_PORT\n",
    )
    .await;
    let message = translate(&wm, "pg")
        .await
        .expect_err("not a port")
        .to_string();
    assert!(message.contains("five-four-three-two"), "{message}");
}
