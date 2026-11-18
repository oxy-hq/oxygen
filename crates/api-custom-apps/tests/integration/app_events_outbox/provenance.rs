//! Platform step 6 (R25(c)): the definer stores the host's provenance MAC
//! beside each row, holding it to 32 bytes, and a store from before the step
//! loses the unsigned signature on its first grant, so a server still calling
//! it fails loudly instead of writing a row nothing signed.

use super::*;

/// One `oxy_events.publish` call carrying `mac` (hex, or none) as its last
/// argument, otherwise as the host makes it.
async fn publish_with_mac(
    client: &tokio_postgres::Client,
    mac: Option<&str>,
) -> Result<Uuid, tokio_postgres::Error> {
    let id = Uuid::new_v4();
    let row = client
        .query_one(
            PUBLISH_SQL,
            &[
                &id,
                &"production",
                &"a.thing",
                &format!("key:{id}"),
                &json!({}),
                &None::<Uuid>,
                &None::<String>,
                &None::<Uuid>,
                &id,
                &0i32,
                &None::<Uuid>,
                &0i32,
                &mac,
            ],
        )
        .await?;
    Ok(row.get(0))
}

/// The definer cannot check a MAC (the key never enters the store), but it
/// takes one of exactly 32 bytes, stores it on the row, and refuses anything
/// else `invalid_parameter_value`, naming the field.
#[tokio::test]
async fn the_definer_stores_a_32_byte_mac_and_refuses_any_other() {
    with_fx(|fx| async move {
        let creds = publishing_writer(&fx, "signed").await;
        let writer = connect(&creds.dsn).await;
        for refused in [
            None,
            Some(hex::encode([1u8; 16])),
            Some(hex::encode([1u8; 33])),
        ] {
            let err = publish_with_mac(&writer, refused.as_deref())
                .await
                .expect_err("a MAC that is not 32 bytes");
            assert_eq!(
                err.code(),
                Some(&SqlState::INVALID_PARAMETER_VALUE),
                "{}",
                rendered(&err)
            );
            assert!(rendered(&err).contains("provenance"), "{}", rendered(&err));
        }
        let owner = owner(&fx).await;
        assert_eq!(outbox_count(&owner).await, 0);

        let mac = [9u8; 32];
        let id = publish_with_mac(&writer, Some(&hex::encode(mac)))
            .await
            .unwrap_or_else(|e| panic!("publish: {}", rendered(&e)));
        let stored: Vec<u8> = owner
            .query_one(
                "SELECT provenance FROM oxy_events.outbox WHERE id = $1",
                &[&id],
            )
            .await
            .expect("read the row")
            .get(0);
        assert_eq!(stored, mac);
    })
    .await;
}

/// A store at version 5 — the outbox without the MAC, a writer granted the
/// unsigned function — gets step 6 on its first grant: the column, the new
/// function, and no unsigned one left to call.
#[tokio::test]
async fn a_version_5_store_loses_the_unsigned_signature_on_first_grant() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let creds = fx
            .ensure_writer(&fx.writer("older"), GrantLevel::ReadWrite)
            .await
            .expect("ensure writer");
        let tenant = fx.tenant_row().await.expect("tenant row");
        let step_five = oxy_oltp::platform::steps_since(4, &tenant.provider, &tenant.database_name)
            .expect("the steps")
            .into_iter()
            .find(|step| step.version == 5)
            .expect("step 5");
        let owner = owner(&fx).await;
        owner
            .batch_execute("DROP SCHEMA oxy_events CASCADE")
            .await
            .expect("drop the outbox");
        for statement in &step_five.statements {
            owner.batch_execute(statement).await.expect("run step 5");
        }
        owner
            .batch_execute(&format!(
                "GRANT USAGE ON SCHEMA oxy_events TO \"{role}\"; \
                 GRANT EXECUTE ON FUNCTION \
                 oxy_events.publish(uuid,text,text,text,jsonb,uuid,text,uuid,uuid,int,uuid,int) \
                 TO \"{role}\"",
                role = creds.role_name
            ))
            .await
            .expect("grant the unsigned function, as a version-5 publish did");
        OltpTenants::update_many()
            .col_expr(oltp_tenants::Column::PlatformSchemaVersion, Expr::value(5))
            .filter(oltp_tenants::Column::OrgId.eq(fx.org_id))
            .exec(&fx.db)
            .await
            .expect("wind the tenant back to version 5");

        events::ensure_publishable(
            &fx.db,
            fx.org_id,
            &OutboxTarget::Production,
            &creds.role_name,
        )
        .await
        .expect("ensure publishable");

        let writer = connect(&creds.dsn).await;
        let id = Uuid::new_v4();
        let unsigned = writer
            .query_one(
                "SELECT oxy_events.publish($1,'production','a.thing','k','{}'::jsonb,NULL,NULL,\
                 NULL,$1,0,NULL,0)",
                &[&id],
            )
            .await
            .expect_err("the unsigned signature is gone");
        assert_eq!(
            unsigned.code(),
            Some(&SqlState::UNDEFINED_FUNCTION),
            "{}",
            rendered(&unsigned)
        );
        publish_with_mac(&writer, Some(&any_mac()))
            .await
            .unwrap_or_else(|e| panic!("the signed publish: {}", rendered(&e)));
        assert_eq!(
            fx.tenant_row()
                .await
                .expect("tenant row")
                .platform_schema_version,
            oxy_oltp::platform::PLATFORM_SCHEMA_VERSION
        );
    })
    .await;
}
