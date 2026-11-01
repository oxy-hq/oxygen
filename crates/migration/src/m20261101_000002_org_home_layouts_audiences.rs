//! Where a home layout's `for` is kept: beside the layout document, never in it.
//! `internal-docs/home-widgets.md` § "Who a widget is for".
//!
//! A server one release back refuses a stored layout carrying a field it does not
//! know, and draws the default home instead. Keeping `for` out of `layout` means
//! that server still reads every org's layout in full and only loses the narrowing.
//! The column carries the document it was split from, so an older server's own save
//! (which leaves this column alone) makes it stale, and it is ignored.
//!
//! Expand-only: one nullable column. A binary one release back never reads it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
ALTER TABLE org_home_layouts ADD COLUMN IF NOT EXISTS audiences JSONB;
"#;

const DOWN_SQL: &str = r#"
ALTER TABLE org_home_layouts DROP COLUMN IF EXISTS audiences;
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP_SQL).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DOWN_SQL)
            .await?;
        Ok(())
    }
}
