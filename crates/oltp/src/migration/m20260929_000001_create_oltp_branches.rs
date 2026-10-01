use sea_orm_migration::prelude::*;

/// The org's staging branch of its OLTP database, and the credentials on it.
///
/// A second migration, NOT folded into the squash: `mod.rs` records that the
/// squash was free only while nothing had run it, and it has run since.
///
/// Both tables are control plane (`data_placement.rs`): where a branch is and
/// the sealed passwords that open it are what Oxy routes a staging `ctx.oltp`
/// call on, before any org data is touched. The branch's rows live in the
/// branch.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
            CREATE TABLE oltp_branches (
                id UUID PRIMARY KEY,
                -- Goes with the tenant. Deprovision deletes the branch at the
                -- provider first; the cascade only tidies the row.
                tenant_row_id UUID NOT NULL
                    REFERENCES oltp_tenants(id) ON DELETE CASCADE,
                kind VARCHAR(16) NOT NULL,
                provider_branch_id VARCHAR(255) NOT NULL,
                parent_branch_id VARCHAR(255) NOT NULL,
                host TEXT NOT NULL,
                database_name VARCHAR(63) NOT NULL,
                owner_role VARCHAR(63) NOT NULL,
                -- NULL where the branch shares the tenant's roles (a local
                -- cluster, where roles are cluster-global).
                owner_password_ciphertext BYTEA,
                status VARCHAR(32) NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                -- Staleness counts from here when set: a reset re-cuts the data.
                last_reset_at TIMESTAMPTZ,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                -- One branch of each kind per org (env design §4.3).
                CONSTRAINT uq_oltp_branches_tenant_kind UNIQUE (tenant_row_id, kind)
            );

            CREATE TABLE oltp_branch_roles (
                id UUID PRIMARY KEY,
                branch_row_id UUID NOT NULL
                    REFERENCES oltp_branches(id) ON DELETE CASCADE,
                role_name VARCHAR(63) NOT NULL,
                password_ciphertext BYTEA NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                CONSTRAINT uq_oltp_branch_roles_branch_role UNIQUE (branch_row_id, role_name)
            );
        "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
            DROP TABLE IF EXISTS oltp_branch_roles;
            DROP TABLE IF EXISTS oltp_branches;
        "#,
            )
            .await?;
        Ok(())
    }
}
