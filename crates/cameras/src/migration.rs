//! SeaORM migrations for the camera fleet domain.
//!
//! Uses a **separate tracking table** (`seaql_migrations_cameras`) so this
//! migrator is fully independent of the central `crates/migration` migrator,
//! matching the agentic pattern documented in `internal-docs/domain-boundaries.md`.
//!
//! Tables owned by this migrator (Postgres only — see
//! `internal-docs/video-processing-fleet-architecture.md` for the Airhouse-side tables):
//!
//!   sites  ←─  edge_boxes
//!                  ↑
//!                  └─  cameras  (edge_box_id is SET NULL)
//!
//! Cross-aggregate ref `sites.workspace_id` is a plain UUID column with no FK
//! constraint (per domain-boundaries.md P3 — Workspace is its own aggregate).

use sea_orm_migration::prelude::*;

pub struct CamerasMigrator;

#[async_trait::async_trait]
impl MigratorTrait for CamerasMigrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(CreateCameraFleetTables),
            Box::new(AddEdgeBoxFunnelHostname),
            Box::new(AddEdgeBoxBandwidthSnapshot),
            Box::new(AddCameraRole),
            Box::new(AddDomainPacks),
            Box::new(AddDeviceRegistry),
            Box::new(AddDeviceClaimSite),
            Box::new(AddEdgeBoxImageTarget),
            Box::new(AddSiteSource),
            Box::new(AddEdgeBoxAuthMode),
            Box::new(CreateAuditEvents),
            Box::new(AddCameraProposedGeometry),
            Box::new(SealDeviceSecrets),
            Box::new(CreateRolloutPlans),
            Box::new(AddEdgeBoxCompatibility),
            Box::new(AddEdgeBoxIncompatibleReason),
            Box::new(AddSitePublicIp),
            Box::new(CreateUnifiCredentials),
            Box::new(CreateComplianceArbitrations),
            Box::new(DropCamerasSiteIdNameUnique),
            Box::new(DropEdgeBoxTokensAndAuthMode),
        ]
    }

    fn migration_table_name() -> sea_orm::DynIden {
        Alias::new("seaql_migrations_cameras").into_iden()
    }

    oxy_migration_tolerance::tolerate_schema_ahead!();
}

// ── Iden enums ──────────────────────────────────────────────────────────────

#[derive(Iden)]
enum Sites {
    #[iden = "sites"]
    Table,
    Id,
    WorkspaceId,
    Name,
    Timezone,
    Region,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum EdgeBoxes {
    #[iden = "edge_boxes"]
    Table,
    Id,
    SiteId,
    HardwareModel,
    ImageTag,
    Cohort,
    TailscaleIp,
    Status,
    UnifiConsoleId,
    UnifiPublicIp,
    UnifiRtspReachable,
    RegisteredAt,
    LastSeenAt,
    UpdatedAt,
}

#[derive(Iden)]
enum Cameras {
    #[iden = "cameras"]
    Table,
    Id,
    SiteId,
    EdgeBoxId,
    Name,
    RtspUrl,
    SubstreamUrl,
    CredentialsRef,
    ZonesJson,
    LinesJson,
    AnalyticsConsent,
    Active,
    ProtectCameraId,
    MacAddress,
    Model,
    Online,
    CreatedAt,
    UpdatedAt,
}

#[derive(Iden)]
enum EdgeBoxTokens {
    #[iden = "edge_box_tokens"]
    Table,
    Id,
    EdgeBoxId,
    TokenHash,
    TokenPrefix,
    Description,
    CreatedAt,
    LastUsedAt,
    RevokedAt,
}

// ── Migration 1: Create tables ──────────────────────────────────────────────

struct CreateCameraFleetTables;

impl MigrationName for CreateCameraFleetTables {
    fn name(&self) -> &str {
        "m20260527_000001_create_camera_fleet_tables"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for CreateCameraFleetTables {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ── sites ──────────────────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(Sites::Table)
                    .if_not_exists()
                    .col(uuid_pk(Sites::Id))
                    .col(ColumnDef::new(Sites::WorkspaceId).uuid().not_null())
                    .col(ColumnDef::new(Sites::Name).text().not_null())
                    .col(
                        ColumnDef::new(Sites::Timezone)
                            .text()
                            .not_null()
                            .default("UTC"),
                    )
                    .col(ColumnDef::new(Sites::Region).text().null())
                    .col(timestamp_default_now(Sites::CreatedAt))
                    .col(timestamp_default_now(Sites::UpdatedAt))
                    .to_owned(),
            )
            .await?;
        // Index for the loose cross-aggregate ref to workspace.
        manager
            .create_index(
                Index::create()
                    .name("sites_workspace_id_idx")
                    .table(Sites::Table)
                    .col(Sites::WorkspaceId)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;

        // ── edge_boxes ─────────────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(EdgeBoxes::Table)
                    .if_not_exists()
                    .col(uuid_pk(EdgeBoxes::Id))
                    .col(ColumnDef::new(EdgeBoxes::SiteId).uuid().not_null())
                    .col(ColumnDef::new(EdgeBoxes::HardwareModel).text().not_null())
                    .col(
                        ColumnDef::new(EdgeBoxes::ImageTag)
                            .text()
                            .not_null()
                            .default("unknown"),
                    )
                    .col(
                        ColumnDef::new(EdgeBoxes::Cohort)
                            .text()
                            .not_null()
                            .default("stable"),
                    )
                    .col(ColumnDef::new(EdgeBoxes::TailscaleIp).text().null())
                    .col(
                        ColumnDef::new(EdgeBoxes::Status)
                            .text()
                            .not_null()
                            .default("pending"),
                    )
                    .col(ColumnDef::new(EdgeBoxes::UnifiConsoleId).text().null())
                    .col(ColumnDef::new(EdgeBoxes::UnifiPublicIp).text().null())
                    .col(
                        ColumnDef::new(EdgeBoxes::UnifiRtspReachable)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(timestamp_default_now(EdgeBoxes::RegisteredAt))
                    .col(
                        ColumnDef::new(EdgeBoxes::LastSeenAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(timestamp_default_now(EdgeBoxes::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_edge_boxes_site")
                            .from(EdgeBoxes::Table, EdgeBoxes::SiteId)
                            .to(Sites::Table, Sites::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("edge_boxes_site_id_idx")
                    .table(EdgeBoxes::Table)
                    .col(EdgeBoxes::SiteId)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;

        // ── cameras ────────────────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(Cameras::Table)
                    .if_not_exists()
                    .col(uuid_pk(Cameras::Id))
                    .col(ColumnDef::new(Cameras::SiteId).uuid().not_null())
                    .col(ColumnDef::new(Cameras::EdgeBoxId).uuid().null())
                    .col(ColumnDef::new(Cameras::Name).text().not_null())
                    .col(ColumnDef::new(Cameras::RtspUrl).text().null())
                    .col(ColumnDef::new(Cameras::SubstreamUrl).text().null())
                    .col(ColumnDef::new(Cameras::CredentialsRef).text().null())
                    .col(jsonb_default_empty_array(Cameras::ZonesJson))
                    .col(jsonb_default_empty_array(Cameras::LinesJson))
                    .col(
                        ColumnDef::new(Cameras::AnalyticsConsent)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(Cameras::Active)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(ColumnDef::new(Cameras::ProtectCameraId).text().null())
                    .col(ColumnDef::new(Cameras::MacAddress).text().null())
                    .col(ColumnDef::new(Cameras::Model).text().null())
                    .col(ColumnDef::new(Cameras::Online).boolean().null())
                    .col(timestamp_default_now(Cameras::CreatedAt))
                    .col(timestamp_default_now(Cameras::UpdatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_cameras_site")
                            .from(Cameras::Table, Cameras::SiteId)
                            .to(Sites::Table, Sites::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_cameras_edge_box")
                            .from(Cameras::Table, Cameras::EdgeBoxId)
                            .to(EdgeBoxes::Table, EdgeBoxes::Id)
                            // Cameras outlive edge box replacement — SET NULL so
                            // the row stays around to be rebound.
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .index(
                        Index::create()
                            .name("cameras_site_id_name_uniq")
                            .table(Cameras::Table)
                            .col(Cameras::SiteId)
                            .col(Cameras::Name)
                            .unique(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("cameras_site_id_idx")
                    .table(Cameras::Table)
                    .col(Cameras::SiteId)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("cameras_edge_box_id_idx")
                    .table(Cameras::Table)
                    .col(Cameras::EdgeBoxId)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;

        // ── edge_box_tokens ────────────────────────────────────────────────
        // token_hash is sha-256 of the bearer; we never store the plaintext.
        // token_prefix is the first 8 chars of the bearer for log visibility.
        manager
            .create_table(
                Table::create()
                    .table(EdgeBoxTokens::Table)
                    .if_not_exists()
                    .col(uuid_pk(EdgeBoxTokens::Id))
                    .col(ColumnDef::new(EdgeBoxTokens::EdgeBoxId).uuid().not_null())
                    .col(
                        ColumnDef::new(EdgeBoxTokens::TokenHash)
                            .text()
                            .not_null()
                            .unique_key(),
                    )
                    .col(ColumnDef::new(EdgeBoxTokens::TokenPrefix).text().not_null())
                    .col(ColumnDef::new(EdgeBoxTokens::Description).text().null())
                    .col(timestamp_default_now(EdgeBoxTokens::CreatedAt))
                    .col(
                        ColumnDef::new(EdgeBoxTokens::LastUsedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(EdgeBoxTokens::RevokedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_edge_box_tokens_box")
                            .from(EdgeBoxTokens::Table, EdgeBoxTokens::EdgeBoxId)
                            .to(EdgeBoxes::Table, EdgeBoxes::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("edge_box_tokens_box_id_idx")
                    .table(EdgeBoxTokens::Table)
                    .col(EdgeBoxTokens::EdgeBoxId)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Drop in reverse FK order.
        for t in [
            EdgeBoxTokens::Table.into_iden(),
            Cameras::Table.into_iden(),
            EdgeBoxes::Table.into_iden(),
            Sites::Table.into_iden(),
        ] {
            manager
                .drop_table(Table::drop().table(t).if_exists().to_owned())
                .await?;
        }
        Ok(())
    }
}

// ── Migration 2: add edge_boxes.funnel_hostname ─────────────────────────────
//
// WebRTC live preview. The
// Funnel hostname is the per-box *.ts.net URL that an operator
// browser dials for WebRTC signaling + TURN-over-TLS media. It's
// nullable because:
//   - Cameras-only deployments may never enable WebRTC.
//   - We populate the column from the edge worker's first
//     authenticated /control/* call (mirrors the tailscale_ip
//     stamping pattern in auth/middleware.rs), so freshly-registered
//     boxes have it NULL until the worker checks in.

struct AddEdgeBoxFunnelHostname;

impl MigrationName for AddEdgeBoxFunnelHostname {
    fn name(&self) -> &str {
        "m20260528_000001_add_edge_box_funnel_hostname"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddEdgeBoxFunnelHostname {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(EdgeBoxes::Table)
                    .add_column(ColumnDef::new(EdgeBoxesV2::FunnelHostname).text().null())
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(EdgeBoxes::Table)
                    .drop_column(EdgeBoxesV2::FunnelHostname)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum EdgeBoxesV2 {
    FunnelHostname,
}

// ── Migration 3: bandwidth snapshot on edge_boxes ───────────────────────────
//
// Per-box "last 5-minute outbound bytes" snapshot, stamped by the
// worker's bandwidth reporter via POST /control/boxes/{id}/bandwidth.
// Surfaces in the UI's edge-boxes table so operators can see who's
// eating their Tailscale Funnel quota before they hit the
// 100 GB/month tailnet cap.
//
// We store on Postgres (not Airhouse) because:
//   - One sample per box every 5 minutes is low cardinality
//   - The UI needs the *latest* value per box, not history. The
//     daily Funnel-attributable byte budget question is a
//     follow-up that probably wants Tailscale's admin API anyway
//     (which gives the precise per-tailnet figure).

struct AddEdgeBoxBandwidthSnapshot;

impl MigrationName for AddEdgeBoxBandwidthSnapshot {
    fn name(&self) -> &str {
        "m20260529_000001_add_edge_box_bandwidth_snapshot"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddEdgeBoxBandwidthSnapshot {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(EdgeBoxes::Table)
                    .add_column(
                        ColumnDef::new(EdgeBoxesV3::Bandwidth5minBytes)
                            .big_integer()
                            .null(),
                    )
                    .add_column(
                        ColumnDef::new(EdgeBoxesV3::BandwidthReportedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(EdgeBoxes::Table)
                    .drop_column(EdgeBoxesV3::Bandwidth5minBytes)
                    .drop_column(EdgeBoxesV3::BandwidthReportedAt)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum EdgeBoxesV3 {
    #[iden = "bandwidth_5min_bytes"]
    Bandwidth5minBytes,
    #[iden = "bandwidth_reported_at"]
    BandwidthReportedAt,
}

// ── AddCameraRole ───────────────────────────────────────────────────────────

/// Adds a free-text `role` column to `cameras`. Used by the edge
/// worker to pick a role-specific VLM prompt: a "kitchen" cam gets a
/// PPE/glove/hairnet prompt, a "dining" cam gets table-state /
/// customer-flow prompts, etc.
///
/// Stored as TEXT (not an enum) on purpose — the prompt registry on
/// the worker treats role as an opaque key with a fallback to
/// "general," so adding new roles doesn't require a schema migration.
/// Defaults to NULL (unset) for existing rows; the worker falls back
/// to the whole-frame prompt when role is missing.
struct AddCameraRole;

impl MigrationName for AddCameraRole {
    fn name(&self) -> &str {
        "m20260528_000001_add_camera_role"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddCameraRole {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Cameras::Table)
                    .add_column(ColumnDef::new(CamerasV2::Role).text().null())
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Cameras::Table)
                    .drop_column(CamerasV2::Role)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum CamerasV2 {
    #[iden = "role"]
    Role,
}

// ── AddDomainPacks ──────────────────────────────────────────────────────────

/// Adds the `camera_domain_packs` table. Domain packs are
/// workspace-scoped bundles of camera roles + VLM prompt templates
/// + output schema.
///
/// Partial unique index enforces exactly one active pack per
/// workspace. The unique `(workspace_id, pack_key)` constraint
/// keeps a workspace's pack list deduped without forcing the
/// active row to have a special primary key shape.
///
/// Does NOT backfill rows here. The service layer seeds the
/// `restaurant` starter pack on first read (or on workspace
/// creation), which keeps the migration idempotent and avoids
/// coupling the migrator to the starter library version.
struct AddDomainPacks;

impl MigrationName for AddDomainPacks {
    fn name(&self) -> &str {
        "m20260528_000002_add_domain_packs"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddDomainPacks {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(DomainPacks::Table)
                    .col(uuid_pk(DomainPacks::Id))
                    .col(ColumnDef::new(DomainPacks::WorkspaceId).uuid().not_null())
                    .col(ColumnDef::new(DomainPacks::PackKey).text().not_null())
                    .col(ColumnDef::new(DomainPacks::Name).text().not_null())
                    .col(ColumnDef::new(DomainPacks::Description).text().null())
                    .col(ColumnDef::new(DomainPacks::StarterKey).text().not_null())
                    .col(
                        ColumnDef::new(DomainPacks::StarterVersion)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DomainPacks::LocallyModified)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(ColumnDef::new(DomainPacks::Body).json_binary().not_null())
                    .col(
                        ColumnDef::new(DomainPacks::IsActive)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(timestamp_default_now(DomainPacks::CreatedAt))
                    .col(timestamp_default_now(DomainPacks::UpdatedAt))
                    .to_owned(),
            )
            .await?;

        // Workspace-scoped pack_key uniqueness.
        manager
            .create_index(
                Index::create()
                    .name("camera_domain_packs_ws_pack_key")
                    .table(DomainPacks::Table)
                    .col(DomainPacks::WorkspaceId)
                    .col(DomainPacks::PackKey)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // Partial unique index: at most one active pack per
        // workspace. SeaORM doesn't have a partial-index builder
        // so we drop to raw SQL.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX camera_domain_packs_one_active_per_ws \
                 ON camera_domain_packs (workspace_id) \
                 WHERE is_active",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DomainPacks::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum DomainPacks {
    #[iden = "camera_domain_packs"]
    Table,
    Id,
    WorkspaceId,
    PackKey,
    Name,
    Description,
    StarterKey,
    StarterVersion,
    LocallyModified,
    Body,
    IsActive,
    CreatedAt,
    UpdatedAt,
}

// ── AddDeviceRegistry ───────────────────────────────────────────────────────

/// IoT Phase 1 — factory-burned identity for self-service onboarding.
///
/// Two tables:
///
///   `device_registry` — one row per physical device ever made.
///   Created by `POST /fleet/factory-enroll`. Persists forever; a
///   returned-to-vendor + re-shipped device keeps the same row.
///
///   `device_claims` — one row per device-workspace binding. Created
///   when the operator scans the QR + picks a workspace. A partial
///   unique index ensures a device has at most one
///   `claimed`/`pending_claim` row at a time.
///
/// Backward compat: existing edge boxes (the operator-typed bearer
/// path) are untouched. The new tables live alongside; phase-1
/// code-paths only fire when a device hits `/fleet/*` endpoints.
struct AddDeviceRegistry;

impl MigrationName for AddDeviceRegistry {
    fn name(&self) -> &str {
        "m20260528_000003_add_device_registry"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddDeviceRegistry {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ── device_registry ────────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(DeviceRegistry::Table)
                    .if_not_exists()
                    // device_id is supplied by the factory enrollment
                    // call, not auto-generated, so no `gen_random_uuid()`
                    // default. We rely on caller-side UUID v7 generation
                    // for sortability + uniqueness.
                    .col(
                        ColumnDef::new(DeviceRegistry::DeviceId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(DeviceRegistry::Sku).text().not_null())
                    .col(
                        ColumnDef::new(DeviceRegistry::HardwareRevision)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DeviceRegistry::ManufacturedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    // Shared secret used for HMAC-SHA256 signing of
                    // /fleet/announce. Stored raw here because HMAC
                    // verification needs the key; an asymmetric
                    // scheme (device holds priv, server holds pub)
                    // would let us store only a pubkey but adds
                    // Jetson TPM-backed keygen complexity we'll
                    // tackle in a later phase. V1 accepts that
                    // server-side compromise leaks all device
                    // secrets (which is already game-over via the
                    // DB anyway). TODO: field-level encryption
                    // with a deployment KEK before any production
                    // deploy.
                    .col(
                        ColumnDef::new(DeviceRegistry::DeviceSecret)
                            .binary()
                            .not_null(),
                    )
                    // Sticky claim code: derived from `(device_id ||
                    // global_claim_salt)` so reprinting a label or
                    // re-enrolling a returned device produces the
                    // same code. Operators see this on the case /
                    // QR sticker.
                    .col(
                        ColumnDef::new(DeviceRegistry::ClaimCode)
                            .text()
                            .not_null()
                            .unique_key(),
                    )
                    // Updated on every successful /fleet/announce.
                    // Drives "device offline" detection in a future
                    // fleet observability tab.
                    .col(
                        ColumnDef::new(DeviceRegistry::LastAnnounceAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(timestamp_default_now(DeviceRegistry::CreatedAt))
                    .to_owned(),
            )
            .await?;

        // ── device_claims ──────────────────────────────────────────────────
        manager
            .create_table(
                Table::create()
                    .table(DeviceClaims::Table)
                    .if_not_exists()
                    .col(uuid_pk(DeviceClaims::Id))
                    .col(ColumnDef::new(DeviceClaims::DeviceId).uuid().not_null())
                    // Loose cross-aggregate ref to workspaces.id, per
                    // domain-boundaries.md P3. No FK.
                    .col(ColumnDef::new(DeviceClaims::WorkspaceId).uuid().not_null())
                    // Set once the device successfully bootstraps —
                    // the edge_boxes row it materializes as. NULL
                    // means the claim is pending (operator clicked
                    // claim but the device hasn't called /fleet/bootstrap
                    // yet).
                    .col(ColumnDef::new(DeviceClaims::EdgeBoxId).uuid().null())
                    // pending_claim | claimed | revoked
                    .col(
                        ColumnDef::new(DeviceClaims::Status)
                            .text()
                            .not_null()
                            .default("pending_claim"),
                    )
                    // user_id who hit the claim landing page. Stored
                    // as plain UUID (no FK to oxy_users) so cameras
                    // stays independent of the auth aggregate.
                    .col(ColumnDef::new(DeviceClaims::ClaimedBy).uuid().null())
                    .col(
                        ColumnDef::new(DeviceClaims::ClaimedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(DeviceClaims::RevokedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(timestamp_default_now(DeviceClaims::CreatedAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_device_claims_device")
                            .from(DeviceClaims::Table, DeviceClaims::DeviceId)
                            .to(DeviceRegistry::Table, DeviceRegistry::DeviceId)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        // Lookup-by-workspace index for the fleet UI.
        manager
            .create_index(
                Index::create()
                    .name("device_claims_workspace_idx")
                    .table(DeviceClaims::Table)
                    .col(DeviceClaims::WorkspaceId)
                    .to_owned(),
            )
            .await?;
        // Partial unique index: at most one non-revoked claim per
        // device. Lets us keep the revoke-history without breaking
        // the unique invariant.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX device_claims_one_active_per_device \
                 ON device_claims (device_id) \
                 WHERE status IN ('pending_claim', 'claimed')",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DeviceClaims::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(DeviceRegistry::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum DeviceRegistry {
    #[iden = "device_registry"]
    Table,
    DeviceId,
    Sku,
    HardwareRevision,
    ManufacturedAt,
    DeviceSecret,
    ClaimCode,
    LastAnnounceAt,
    CreatedAt,
}

#[derive(Iden)]
enum DeviceClaims {
    #[iden = "device_claims"]
    Table,
    Id,
    DeviceId,
    WorkspaceId,
    EdgeBoxId,
    Status,
    ClaimedBy,
    ClaimedAt,
    RevokedAt,
    CreatedAt,
}

// ── AddDeviceClaimSite ──────────────────────────────────────────────────────

/// Adds `site_id` to `device_claims` so the operator picks which
/// site the device will materialize under *at claim time* — the
/// bootstrap path then has everything it needs to create the
/// `edge_boxes` row without a second operator round-trip.
///
/// Nullable: pending claims created without a site explicitly
/// (e.g. UI defaults to "create new site at bootstrap") get NULL,
/// and the bootstrap service errors loudly. The phase 1B operator
/// route requires the field.
struct AddDeviceClaimSite;

impl MigrationName for AddDeviceClaimSite {
    fn name(&self) -> &str {
        "m20260528_000004_add_device_claim_site"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddDeviceClaimSite {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(DeviceClaims::Table)
                    .add_column(ColumnDef::new(DeviceClaimsV2::SiteId).uuid().null())
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(DeviceClaims::Table)
                    .drop_column(DeviceClaimsV2::SiteId)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum DeviceClaimsV2 {
    #[iden = "site_id"]
    SiteId,
}

// ── AddEdgeBoxImageTarget ───────────────────────────────────────────────────

/// IoT Phase 2 — software-update channel columns on `edge_boxes`.
///
/// Five fields:
/// * `target_image_tag` — operator-set "we want this box on this
///   image." NULL = no automated update (legacy mode where operator
///   ssh-pulls).
/// * `current_image_tag` — last-reported running image from the
///   device's `/control/boxes/{id}/health` ping.
/// * `held_until` — pause-window. If set + in the future, the update
///   agent treats target as if it equals current ("don't touch this
///   box during the lunch rush").
/// * `last_update_result` — `applied` | `reverted` | `in_progress`.
///   Operator-facing state badge for the fleet UI.
/// * `last_update_at` — timestamp of the last attempt for debugging
///   stuck updates.
///
/// All nullable so existing edge boxes deployed before this
/// migration keep working unchanged.
struct AddEdgeBoxImageTarget;

impl MigrationName for AddEdgeBoxImageTarget {
    fn name(&self) -> &str {
        "m20260528_000005_add_edge_box_image_target"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddEdgeBoxImageTarget {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(EdgeBoxes::Table)
                    .add_column(ColumnDef::new(EdgeBoxesV4::TargetImageTag).text().null())
                    .add_column(ColumnDef::new(EdgeBoxesV4::CurrentImageTag).text().null())
                    .add_column(
                        ColumnDef::new(EdgeBoxesV4::HeldUntil)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .add_column(ColumnDef::new(EdgeBoxesV4::LastUpdateResult).text().null())
                    .add_column(
                        ColumnDef::new(EdgeBoxesV4::LastUpdateAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(EdgeBoxes::Table)
                    .drop_column(EdgeBoxesV4::TargetImageTag)
                    .drop_column(EdgeBoxesV4::CurrentImageTag)
                    .drop_column(EdgeBoxesV4::HeldUntil)
                    .drop_column(EdgeBoxesV4::LastUpdateResult)
                    .drop_column(EdgeBoxesV4::LastUpdateAt)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum EdgeBoxesV4 {
    #[iden = "target_image_tag"]
    TargetImageTag,
    #[iden = "current_image_tag"]
    CurrentImageTag,
    #[iden = "held_until"]
    HeldUntil,
    #[iden = "last_update_result"]
    LastUpdateResult,
    #[iden = "last_update_at"]
    LastUpdateAt,
}

// ── AddSiteSource ───────────────────────────────────────────────────────────

/// Adds `source` to `sites` so manual + UniFi-discovered rows can
/// coexist. NEW rows default to `manual`. Backfill: any site that
/// has at least one edge_box with `unifi_console_id IS NOT NULL`
/// is marked `unifi` — that's the only signal of provenance on
/// the current schema (sites don't directly carry a UniFi id).
///
/// The column drives two behaviors:
///   1. UI shows a "UniFi" / "Manual" badge per site.
///   2. The UniFi import upsert filters its UPDATE to
///      `source = 'unifi'` so a re-import never clobbers a manual
///      site that happens to share a name with a UniFi-discovered one.
struct AddSiteSource;

impl MigrationName for AddSiteSource {
    fn name(&self) -> &str {
        "m20260529_000001_add_site_source"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddSiteSource {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Sites::Table)
                    .add_column(
                        ColumnDef::new(SitesV2::Source)
                            .text()
                            .not_null()
                            .default("manual"),
                    )
                    .to_owned(),
            )
            .await?;
        // Backfill: sites whose edge_boxes carry a unifi_console_id
        // are UniFi-discovered. Direct JOIN via subquery — sites
        // table itself has no UniFi marker.
        manager
            .get_connection()
            .execute_unprepared(
                "UPDATE sites SET source = 'unifi' \
                 WHERE id IN ( \
                     SELECT DISTINCT site_id FROM edge_boxes \
                     WHERE unifi_console_id IS NOT NULL \
                 )",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Sites::Table)
                    .drop_column(SitesV2::Source)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum SitesV2 {
    #[iden = "source"]
    Source,
}

/// Adds `auth_mode` to `edge_boxes` (IoT Phase 3). Drives operator
/// visibility into the bearer → JWT rollout: the auth middleware
/// stamps the column on every successful auth so the dashboard
/// can show "which boxes still need to migrate." Default
/// `'bearer'` keeps existing rows in the legacy mode until JWT
/// auth succeeds for them.
struct AddEdgeBoxAuthMode;

impl MigrationName for AddEdgeBoxAuthMode {
    fn name(&self) -> &str {
        "m20260530_000001_add_edge_box_auth_mode"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddEdgeBoxAuthMode {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(EdgeBoxes::Table)
                    .add_column(
                        ColumnDef::new(EdgeBoxesV5::AuthMode)
                            .string_len(16)
                            .not_null()
                            .default("bearer"),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(EdgeBoxes::Table)
                    .drop_column(EdgeBoxesV5::AuthMode)
                    .to_owned(),
            )
            .await
    }
}

#[derive(Iden)]
enum EdgeBoxesV5 {
    #[iden = "auth_mode"]
    AuthMode,
}

/// Operator-action audit log (Tier B #7). One row per mutating
/// operator action so the dashboard can answer "who set what
/// target, who claimed which device, when." Cross-aggregate
/// `actor_user_id` is a loose UUID column (no FK) following the
/// existing convention (`sites.workspace_id`, `device_claims.
/// claimed_by`); user lifecycle is owned by the auth aggregate
/// and we don't want to cascade-delete audit on user deletion
/// anyway.
struct CreateAuditEvents;

impl MigrationName for CreateAuditEvents {
    fn name(&self) -> &str {
        "m20260530_000002_create_camera_audit_events"
    }
}

#[derive(Iden)]
enum AuditEvents {
    #[iden = "camera_audit_events"]
    Table,
    Id,
    WorkspaceId,
    ActorUserId,
    Action,
    TargetId,
    DetailsJson,
    CreatedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for CreateAuditEvents {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AuditEvents::Table)
                    .if_not_exists()
                    .col(uuid_pk(AuditEvents::Id))
                    .col(ColumnDef::new(AuditEvents::WorkspaceId).uuid().not_null())
                    .col(ColumnDef::new(AuditEvents::ActorUserId).uuid().null())
                    // Free-text action key (e.g. `edge_box.set_target`,
                    // `device.claim`). TEXT not enum so a new action
                    // doesn't need a migration.
                    .col(ColumnDef::new(AuditEvents::Action).text().not_null())
                    // The thing being acted on — edge_box_id, device_id,
                    // claim_id depending on action. NULL when the
                    // action is workspace-wide.
                    .col(ColumnDef::new(AuditEvents::TargetId).uuid().null())
                    // Action-specific context. `{"target_image_tag":
                    // "v1.2.3"}` for set-target; `{"device_id": "...",
                    // "edge_box_id": "..."}` for binds; etc. Operators
                    // see the rendered JSON in the table.
                    .col(
                        ColumnDef::new(AuditEvents::DetailsJson)
                            .json_binary()
                            .not_null()
                            .default(Expr::cust("'{}'::jsonb")),
                    )
                    .col(timestamp_default_now(AuditEvents::CreatedAt))
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_audit_events_workspace_created")
                    .table(AuditEvents::Table)
                    .col(AuditEvents::WorkspaceId)
                    .col(AuditEvents::CreatedAt)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AuditEvents::Table).to_owned())
            .await
    }
}

/// Adds `proposed_zones_json` + `proposed_lines_json` to
/// `cameras` so an operator can review auto-proposed geometry
/// before it goes live. The `proposed_at` timestamp lets the
/// UI surface "proposed 3h ago" so stale proposals look
/// untrustworthy. All three columns are nullable — `NULL` means
/// "no proposal pending," distinct from "proposed an empty
/// array" (which would clear the live zones on approval).
struct AddCameraProposedGeometry;

impl MigrationName for AddCameraProposedGeometry {
    fn name(&self) -> &str {
        "m20260530_000003_add_camera_proposed_geometry"
    }
}

#[derive(Iden)]
enum CamerasV3 {
    #[iden = "proposed_zones_json"]
    ProposedZonesJson,
    #[iden = "proposed_lines_json"]
    ProposedLinesJson,
    #[iden = "proposed_at"]
    ProposedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for AddCameraProposedGeometry {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Cameras::Table)
                    .add_column(
                        ColumnDef::new(CamerasV3::ProposedZonesJson)
                            .json_binary()
                            .null(),
                    )
                    .add_column(
                        ColumnDef::new(CamerasV3::ProposedLinesJson)
                            .json_binary()
                            .null(),
                    )
                    .add_column(
                        ColumnDef::new(CamerasV3::ProposedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Cameras::Table)
                    .drop_column(CamerasV3::ProposedZonesJson)
                    .drop_column(CamerasV3::ProposedLinesJson)
                    .drop_column(CamerasV3::ProposedAt)
                    .to_owned(),
            )
            .await
    }
}

/// Seals every existing `device_registry.device_secret` row at rest.
///
/// The column type stays `BYTEA`; the contents go from raw 32-byte
/// secrets to `[version_byte, nonce, ciphertext]` envelopes (see
/// `crate::secrets`). Service-layer reads added in the same change
/// expect every row to be sealed, so this migration must complete
/// before the new binary serves traffic.
///
/// Idempotent: rows that already start with the seal version byte
/// are left alone, so a partially-applied migration can be retried.
///
/// Operates row-by-row via the raw `query_one` API rather than
/// SeaORM entities so it doesn't depend on the entity layer matching
/// the schema at this migration version.
struct SealDeviceSecrets;

impl MigrationName for SealDeviceSecrets {
    fn name(&self) -> &str {
        "m20260530_000004_seal_device_secrets"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for SealDeviceSecrets {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        use sea_orm::Statement;
        let db = manager.get_connection();
        let backend = db.get_database_backend();

        // Pull every device + secret. Small table — the entire
        // device_registry is bounded by "all devices we've ever
        // manufactured," so an in-memory pass is fine.
        let rows = db
            .query_all_raw(Statement::from_string(
                backend,
                "SELECT device_id, device_secret FROM device_registry".to_string(),
            ))
            .await?;

        for row in rows {
            let device_id: uuid::Uuid = row.try_get("", "device_id")?;
            let secret: Vec<u8> = row.try_get("", "device_secret")?;
            if crate::secrets::looks_sealed(&secret) {
                continue;
            }
            let sealed = crate::secrets::seal(&secret)
                .map_err(|e| DbErr::Custom(format!("seal device_secret for {device_id}: {e}")))?;
            db.execute_raw(Statement::from_sql_and_values(
                backend,
                "UPDATE device_registry SET device_secret = $1 WHERE device_id = $2",
                vec![sealed.into(), device_id.into()],
            ))
            .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        use sea_orm::Statement;
        let db = manager.get_connection();
        let backend = db.get_database_backend();
        let rows = db
            .query_all_raw(Statement::from_string(
                backend,
                "SELECT device_id, device_secret FROM device_registry".to_string(),
            ))
            .await?;

        for row in rows {
            let device_id: uuid::Uuid = row.try_get("", "device_id")?;
            let secret: Vec<u8> = row.try_get("", "device_secret")?;
            if !crate::secrets::looks_sealed(&secret) {
                continue;
            }
            let opened = crate::secrets::open(&secret)
                .map_err(|e| DbErr::Custom(format!("open device_secret for {device_id}: {e}")))?;
            db.execute_raw(Statement::from_sql_and_values(
                backend,
                "UPDATE device_registry SET device_secret = $1 WHERE device_id = $2",
                vec![opened.into(), device_id.into()],
            ))
            .await?;
        }
        Ok(())
    }
}

/// Canary rollout plans for OTA target_image_tag changes. See
/// `service::rollouts` for the state machine. Indexed on
/// `(workspace_id, status)` so the supervisor can cheaply find
/// active plans across all workspaces.
struct CreateRolloutPlans;

impl MigrationName for CreateRolloutPlans {
    fn name(&self) -> &str {
        "m20260530_000005_create_rollout_plans"
    }
}

#[derive(Iden)]
enum RolloutPlans {
    #[iden = "camera_rollout_plans"]
    Table,
    Id,
    WorkspaceId,
    TargetImageTag,
    CanaryCohort,
    CanaryMinMinutes,
    FailureThresholdPct,
    Status,
    CanaryStartedAt,
    PromotionStartedAt,
    CompletedAt,
    AbortedAt,
    AbortReason,
    ActorUserId,
    CreatedAt,
    UpdatedAt,
}

#[async_trait::async_trait]
impl MigrationTrait for CreateRolloutPlans {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(RolloutPlans::Table)
                    .col(uuid_pk(RolloutPlans::Id))
                    .col(ColumnDef::new(RolloutPlans::WorkspaceId).uuid().not_null())
                    .col(
                        ColumnDef::new(RolloutPlans::TargetImageTag)
                            .text()
                            .not_null(),
                    )
                    .col(ColumnDef::new(RolloutPlans::CanaryCohort).text().not_null())
                    .col(
                        ColumnDef::new(RolloutPlans::CanaryMinMinutes)
                            .integer()
                            .not_null()
                            .default(1440),
                    )
                    .col(
                        ColumnDef::new(RolloutPlans::FailureThresholdPct)
                            .float()
                            .not_null()
                            .default(5.0),
                    )
                    .col(
                        ColumnDef::new(RolloutPlans::Status)
                            .text()
                            .not_null()
                            .default("pending"),
                    )
                    .col(
                        ColumnDef::new(RolloutPlans::CanaryStartedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(RolloutPlans::PromotionStartedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(RolloutPlans::CompletedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(RolloutPlans::AbortedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(ColumnDef::new(RolloutPlans::AbortReason).text().null())
                    .col(ColumnDef::new(RolloutPlans::ActorUserId).uuid().null())
                    .col(timestamp_default_now(RolloutPlans::CreatedAt))
                    .col(timestamp_default_now(RolloutPlans::UpdatedAt))
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_rollout_plans_workspace_status")
                    .table(RolloutPlans::Table)
                    .col(RolloutPlans::WorkspaceId)
                    .col(RolloutPlans::Status)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(RolloutPlans::Table).to_owned())
            .await
    }
}

/// Adds `edge_compatibility_json` to `edge_boxes` (CI #3). Stores the
/// raw manifest the worker reads from `/opt/oxy-edge/compatibility.json`
/// inside its container. Nullable so old workers (built before this
/// column existed) keep posting health successfully — server-side
/// gates on protocol_version are a separate follow-up.
struct AddEdgeBoxCompatibility;

impl MigrationName for AddEdgeBoxCompatibility {
    fn name(&self) -> &str {
        "m20260531_000001_add_edge_box_compatibility"
    }
}

#[derive(Iden)]
enum EdgeBoxesV6 {
    #[iden = "edge_compatibility_json"]
    EdgeCompatibilityJson,
}

#[async_trait::async_trait]
impl MigrationTrait for AddEdgeBoxCompatibility {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(EdgeBoxes::Table)
                    .add_column(
                        ColumnDef::new(EdgeBoxesV6::EdgeCompatibilityJson)
                            .json_binary()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(EdgeBoxes::Table)
                    .drop_column(EdgeBoxesV6::EdgeCompatibilityJson)
                    .to_owned(),
            )
            .await
    }
}

/// Adds `incompatible_reason` to `edge_boxes` (P1 #3 follow-up).
/// Populated by `record_health_update` when the worker's reported
/// `protocol_version` is below `OXY_MIN_EDGE_PROTOCOL_VERSION`.
/// NULL means "compatible" or "no manifest posted yet."
struct AddEdgeBoxIncompatibleReason;

impl MigrationName for AddEdgeBoxIncompatibleReason {
    fn name(&self) -> &str {
        "m20260531_000002_add_edge_box_incompatible_reason"
    }
}

#[derive(Iden)]
enum EdgeBoxesV7 {
    #[iden = "incompatible_reason"]
    IncompatibleReason,
}

#[async_trait::async_trait]
impl MigrationTrait for AddEdgeBoxIncompatibleReason {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(EdgeBoxes::Table)
                    .add_column(
                        ColumnDef::new(EdgeBoxesV7::IncompatibleReason)
                            .text()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(EdgeBoxes::Table)
                    .drop_column(EdgeBoxesV7::IncompatibleReason)
                    .to_owned(),
            )
            .await
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn uuid_pk<I: IntoIden + 'static>(name: I) -> ColumnDef {
    ColumnDef::new(name)
        .uuid()
        .not_null()
        .default(Expr::cust("gen_random_uuid()"))
        .primary_key()
        .to_owned()
}

fn timestamp_default_now<I: IntoIden + 'static>(name: I) -> ColumnDef {
    ColumnDef::new(name)
        .timestamp_with_time_zone()
        .not_null()
        .default(Expr::current_timestamp())
        .to_owned()
}

fn jsonb_default_empty_array<I: IntoIden + 'static>(name: I) -> ColumnDef {
    ColumnDef::new(name)
        .json_binary()
        .not_null()
        .default(Expr::cust("'[]'::jsonb"))
        .to_owned()
}

// ── AddSitePublicIp ─────────────────────────────────────────────────────────

/// Adds `public_ip` to `sites` so operators can paste a customer's
/// WAN-facing IP and use it to rewrite each camera's `rtsp_url` to
/// the externally-reachable UniFi port (7447) before the edge box is
/// installed on-site. Bridges the gap between "demo signed, customer
/// available" and "edge hardware arrives": the backend can pull RTSP
/// from the public IP and run compliance until the box ships.
///
/// Nullable because most sites either have a box on premises (LAN
/// access via tailscale_ip, public IP irrelevant) or are pre-edge-box
/// in their demo phase. NULL means "no NAT path configured" and the
/// bulk-rewrite endpoint refuses to run until it's set.
struct AddSitePublicIp;

impl MigrationName for AddSitePublicIp {
    fn name(&self) -> &str {
        "m20260601_000001_add_site_public_ip"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddSitePublicIp {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Sites::Table)
                    .add_column(ColumnDef::new(SitesV3::PublicIp).text().null())
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Sites::Table)
                    .drop_column(SitesV3::PublicIp)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum SitesV3 {
    #[iden = "public_ip"]
    PublicIp,
}

// ── CreateUnifiCredentials ─────────────────────────────────────────────────

/// Per-workspace pointer to a stored UniFi Site Manager API key.
/// The key plaintext itself lives in `org_secrets` (encrypted at rest
/// with the deployment master key); this table only carries the
/// (workspace_id → secret_id) lookup + audit metadata.
///
/// Workspace-scoped (UNIQUE on workspace_id) because UniFi accounts
/// map 1:1 to customer demos. The same operator may run multiple
/// customer demos in different workspaces and each has its own key.
/// `org_id` is denormalized for the secret lookup so we don't have
/// to re-join `workspaces` every time we resolve the credential.
struct CreateUnifiCredentials;

impl MigrationName for CreateUnifiCredentials {
    fn name(&self) -> &str {
        "m20260601_000002_create_unifi_credentials"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for CreateUnifiCredentials {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(UnifiCredentials::Table)
                    .if_not_exists()
                    .col(uuid_pk(UnifiCredentials::Id))
                    .col(
                        ColumnDef::new(UnifiCredentials::WorkspaceId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(UnifiCredentials::OrgId).uuid().not_null())
                    .col(ColumnDef::new(UnifiCredentials::SecretId).uuid().not_null())
                    .col(ColumnDef::new(UnifiCredentials::SetByUserId).uuid().null())
                    .col(timestamp_default_now(UnifiCredentials::CreatedAt))
                    .col(timestamp_default_now(UnifiCredentials::UpdatedAt))
                    .to_owned(),
            )
            .await?;
        // Workspace uniqueness — one stored key per workspace. A
        // re-PUT upserts on the existing row.
        manager
            .create_index(
                Index::create()
                    .name("unifi_credentials_workspace_unique")
                    .table(UnifiCredentials::Table)
                    .col(UnifiCredentials::WorkspaceId)
                    .unique()
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(UnifiCredentials::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum UnifiCredentials {
    #[iden = "unifi_credentials"]
    Table,
    Id,
    WorkspaceId,
    OrgId,
    SecretId,
    SetByUserId,
    CreatedAt,
    UpdatedAt,
}

// ── CreateComplianceArbitrations ───────────────────────────────────────────

/// Operator-arbitrated verdicts for compliance events where the VLM
/// and YOLO disagreed (Phase 1+2 of "utilizing disagreements"). One
/// row per (report_id, arbiter) — re-arbitrating updates the row in
/// place, so the table holds the operator's CURRENT belief, not a
/// history. The compliance_reports row in Airhouse is the immutable
/// raw input; arbitrations are the labeled training-data overlay.
///
/// `report_id` is a loose VARCHAR ref into Airhouse (no FK — those
/// rows live in a different database). Workspace ownership is
/// enforced at the route layer before insert.
struct CreateComplianceArbitrations;

impl MigrationName for CreateComplianceArbitrations {
    fn name(&self) -> &str {
        "m20260601_000003_create_compliance_arbitrations"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for CreateComplianceArbitrations {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ComplianceArbitrations::Table)
                    .if_not_exists()
                    .col(uuid_pk(ComplianceArbitrations::Id))
                    .col(
                        ColumnDef::new(ComplianceArbitrations::WorkspaceId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ComplianceArbitrations::ReportId)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ComplianceArbitrations::ArbiterUserId)
                            .uuid()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ComplianceArbitrations::Verdict)
                            .text()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ComplianceArbitrations::Notes).text().null())
                    .col(timestamp_default_now(ComplianceArbitrations::CreatedAt))
                    .col(timestamp_default_now(ComplianceArbitrations::UpdatedAt))
                    .to_owned(),
            )
            .await?;
        // One arbitration per report — re-arbitrate updates. We don't
        // model multi-operator review yet; v0 assumes one verdict per
        // event is fine and the UI surfaces "last arbiter wins."
        manager
            .create_index(
                Index::create()
                    .name("compliance_arbitrations_report_unique")
                    .table(ComplianceArbitrations::Table)
                    .col(ComplianceArbitrations::ReportId)
                    .unique()
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;
        // Lookup by workspace for the operator UI's "all arbitrations
        // in my workspace" view.
        manager
            .create_index(
                Index::create()
                    .name("compliance_arbitrations_workspace_idx")
                    .table(ComplianceArbitrations::Table)
                    .col(ComplianceArbitrations::WorkspaceId)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(ComplianceArbitrations::Table)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(Iden)]
enum ComplianceArbitrations {
    #[iden = "compliance_arbitrations"]
    Table,
    Id,
    WorkspaceId,
    ReportId,
    ArbiterUserId,
    Verdict,
    Notes,
    CreatedAt,
    UpdatedAt,
}

// ── DropCamerasSiteIdNameUnique ────────────────────────────────────────────

/// UniFi customers routinely run multiple cameras with the same
/// display name (e.g. a row of "Cooler" cameras pointed at four
/// different walk-ins). The legacy `cameras_site_id_name_uniq` index
/// silently turned the second import's INSERT into an UPDATE against
/// the first row's PK — losing the additional cameras from the UI.
///
/// Cameras already have:
///   - `id` (primary key, deterministic per-UniFi-device-id for imports)
///   - `protect_camera_id` (UniFi's stable id)
///
/// Either is a real unique identifier; the legacy `(site_id, name)`
/// constraint conflated display-string with identity. We drop it.
/// `service::onboarding` now uses `OnConflict::column(Cameras::Id)`
/// for the upsert (the same deterministic id was always the source of
/// truth — the secondary index was an over-eager guard).
///
/// The boundary check in `service::provisioning::create_camera` keeps
/// the "no two manually-created cameras with the same name in a site"
/// guarantee for the operator-driven path.
struct DropCamerasSiteIdNameUnique;

impl MigrationName for DropCamerasSiteIdNameUnique {
    fn name(&self) -> &str {
        "m20260601_000004_drop_cameras_site_id_name_unique"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for DropCamerasSiteIdNameUnique {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // The original `CreateCameras` migration declared this via
        // `.index(Index::create().unique())` inline on `Table::create()`,
        // which sea-orm's Postgres backend lands as a UNIQUE *constraint*
        // (with an implicit backing index of the same name). `Index::drop`
        // alone fails with "cannot drop index ... because constraint ...
        // requires it" — the constraint owns the index. Drop the
        // constraint instead; Postgres tears down its backing index in
        // the same statement. `IF EXISTS` keeps this safe to re-run.
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE cameras DROP CONSTRAINT IF EXISTS cameras_site_id_name_uniq",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Mirror the original create-shape: a UNIQUE constraint, so
        // a future `down` of this migration can be undone again by
        // the same `ALTER TABLE DROP CONSTRAINT` path. Rollback fails
        // if any duplicate-name rows have been imported since `up` ran
        // — that's the correct behavior (rollback should refuse
        // rather than silently delete data).
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE cameras ADD CONSTRAINT cameras_site_id_name_uniq UNIQUE (site_id, name)",
            )
            .await?;
        Ok(())
    }
}

/// Final bearer-purge: drop the `edge_box_tokens` table and the
/// `edge_boxes.auth_mode` column. Middleware has been JWT-only since
/// the 2026-06 cutover; no production fleet remains on the bearer
/// path, so the table + column are dead schema.
///
/// Cascade order: drop `edge_box_tokens` first (it has an FK to
/// `edge_boxes`), then drop the column.
struct DropEdgeBoxTokensAndAuthMode;

impl MigrationName for DropEdgeBoxTokensAndAuthMode {
    fn name(&self) -> &str {
        "m20260608_000001_drop_edge_box_tokens_and_auth_mode"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for DropEdgeBoxTokensAndAuthMode {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(EdgeBoxTokens::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(EdgeBoxes::Table)
                    .drop_column(EdgeBoxesV5::AuthMode)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Restore the column with its original default so an
        // emergency rollback works. We do NOT recreate
        // `edge_box_tokens` — the table dropped above was empty by
        // the time of cutover and rebuilding it would require
        // re-issuing every bearer (which the system no longer knows
        // how to do).
        manager
            .alter_table(
                Table::alter()
                    .table(EdgeBoxes::Table)
                    .add_column(
                        ColumnDef::new(EdgeBoxesV5::AuthMode)
                            .string_len(16)
                            .not_null()
                            .default("jwt"),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
