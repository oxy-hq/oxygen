use sea_orm_migration::prelude::*;

/// A custom-app environment's own branch of the org's OLTP database, and the
/// credentials on it.
///
/// Separate tables from `oltp_branches` on purpose: a reverted binary decodes
/// every `oltp_branches.kind`, so a new kind there would break a rollback. A
/// new table is simply unknown to the older binary.
///
/// Both tables are control plane (`data_placement.rs`): where an environment's
/// branch is, which point of production it was cut at, and the sealed passwords
/// that open it are what Oxy routes a sandbox `ctx.oltp` call on, before any
/// org data is touched. The branch's rows live in the branch.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
            CREATE TABLE oltp_env_branches (
                id UUID PRIMARY KEY,
                -- Goes with the tenant. Deprovision deletes the branch at the
                -- provider first; the cascade only tidies the row.
                tenant_row_id UUID NOT NULL
                    REFERENCES oltp_tenants(id) ON DELETE CASCADE,
                -- No FK to apps: an app delete must not drop the row of a live
                -- Neon branch. The provider's expires_at backstops orphans.
                app_id UUID NOT NULL,
                env_name VARCHAR(64) NOT NULL,
                -- 'branch' (forked from the parent) or 'empty' (schema only).
                mode VARCHAR(16) NOT NULL,
                -- cutting | migrating | ready | resetting | failed
                status VARCHAR(32) NOT NULL,
                error TEXT,
                -- NULL until the provider has created the branch.
                provider_branch_id VARCHAR(255),
                parent_branch_id VARCHAR(255) NOT NULL,
                host TEXT,
                database_name VARCHAR(63),
                owner_role VARCHAR(63),
                -- NULL where the branch shares the tenant's roles (a local
                -- cluster, where roles are cluster-global).
                owner_password_ciphertext BYTEA,
                -- The point of the parent the branch was cut at.
                fork_at TIMESTAMPTZ NOT NULL,
                expires_at TIMESTAMPTZ NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                last_reset_at TIMESTAMPTZ,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                CONSTRAINT uq_oltp_env_branches_tenant_app_env
                    UNIQUE (tenant_row_id, app_id, env_name)
            );

            CREATE TABLE oltp_env_branch_roles (
                id UUID PRIMARY KEY,
                env_branch_row_id UUID NOT NULL
                    REFERENCES oltp_env_branches(id) ON DELETE CASCADE,
                role_name VARCHAR(63) NOT NULL,
                password_ciphertext BYTEA NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                CONSTRAINT uq_oltp_env_branch_roles_branch_role
                    UNIQUE (env_branch_row_id, role_name)
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
            DROP TABLE IF EXISTS oltp_env_branch_roles;
            DROP TABLE IF EXISTS oltp_env_branches;
        "#,
            )
            .await?;
        Ok(())
    }
}
