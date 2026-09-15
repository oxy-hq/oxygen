//! The vocabulary: what can go wrong, what was declared, what was applied.

/// `custom_app_migrations.store` for a file applied to the app's OLTP schema.
pub(crate) const STORE_OLTP: &str = "oltp";
/// `custom_app_migrations.store` for a file applied to the app's Airhouse schema.
pub(crate) const STORE_AIRHOUSE: &str = "airhouse";

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error("{0}")]
    BadManifest(String),
    #[error(
        "the `migrations.dir` {0:?} in oxy-app.json is not a safe path inside the bundle — \
         use a plain relative directory such as \"migrations\""
    )]
    UnsafeDir(String),
    /// Declaring a directory the bundle does not populate is almost always a
    /// typo or a build that forgot to copy the files. Shipping the app anyway
    /// would land as `relation does not exist` on a user, so it is refused.
    #[error(
        "oxy-app.json declares migrations in {dir:?} but the bundle carries no `.sql` files \
         there — check the directory name and that your build copies it into the bundle"
    )]
    EmptyDir { dir: String },
    #[error("migration {filename:?} is not valid UTF-8 text: {message}")]
    NotUtf8 { filename: String, message: String },
    /// THE rule. See the module docs.
    #[error(
        "migration {filename:?} was already applied to this app with different contents \
         (recorded {applied}, bundle has {bundled}). Editing a migration that has already run \
         diverges this app's database from what the file says — add a NEW migration file \
         instead."
    )]
    ChecksumMismatch {
        filename: String,
        applied: String,
        bundled: String,
    },
    /// The same rule reached by the other route: an author who renames an
    /// applied file gets a new ledger key and would re-run it. That is precisely
    /// how the launcher-plan row got duplicated, so it is refused by content.
    #[error(
        "migration {filename:?} has the same contents as {applied_as:?}, which this app has \
         already applied — renaming or copying an applied migration would run it a second time. \
         Restore the original name, or write a new migration that makes the change you want."
    )]
    AlreadyAppliedUnderAnotherName {
        filename: String,
        applied_as: String,
    },
    #[error(
        "this app declares schema migrations but its OLTP schema could not be resolved: {0}. \
         Ask whoever operates this org to provision the app's OLTP store."
    )]
    NoSchema(String),
    #[error("could not connect to the app's OLTP database: {0}")]
    Connect(String),
    #[error(
        "another promote is applying this app's migrations — wait for it to finish and \
         re-publish"
    )]
    Busy,
    #[error("migration {filename:?} failed: {message}")]
    Failed { filename: String, message: String },
    /// An Airhouse migration breaks a rule checked before anything runs: an
    /// object outside the app's schema, or a key, `UNIQUE`, index or foreign key
    /// DuckLake cannot hold.
    #[error("Airhouse migration {filename:?} was refused: {message}")]
    AirhouseRule { filename: String, message: String },
    /// The tenant connection or transaction machinery failed around a file —
    /// beginning a transaction, or committing one.
    ///
    /// Split from [`MigrationError::Failed`] because the two are opposite
    /// answers to "whose problem is this". `Failed` is the file's SQL being
    /// wrong: the author fixes it and re-publishes, and a retry without a change
    /// is pointless. This one is ours — a dropped connection, a tenant restart —
    /// and a retry is exactly the right response. Reporting it as author fault
    /// told CI "your change is wrong" about a blip, and reporting the reverse
    /// makes a permanently broken migration look like a flake worth retrying
    /// forever.
    #[error("the app store was unreachable while applying {filename}: {message}")]
    Infra { filename: String, message: String },
    #[error(
        "migration {filename:?} applied but could not be recorded in the ledger ({message}); \
         re-publishing will attempt it again, which will fail loudly rather than run it twice"
    )]
    LedgerWriteFailed { filename: String, message: String },
    #[error("database error: {0}")]
    Db(String),
}

impl MigrationError {
    /// Whether the publisher can fix this by changing the bundle.
    ///
    /// Drives the HTTP status: a 4xx tells CI "your change is wrong", a 5xx
    /// tells it "retry". Getting this backwards makes a permanently broken
    /// migration look like a flake worth retrying forever.
    pub fn is_author_fault(&self) -> bool {
        matches!(
            self,
            MigrationError::BadManifest(_)
                | MigrationError::UnsafeDir(_)
                | MigrationError::EmptyDir { .. }
                | MigrationError::NotUtf8 { .. }
                | MigrationError::ChecksumMismatch { .. }
                | MigrationError::AlreadyAppliedUnderAnotherName { .. }
                // `NoSchema` stays author fault: it is reached only when the
                // app's own SLUG cannot back a schema name, which is a bundle
                // fact the publisher controls. The store being unprovisioned is
                // `Infra`, deliberately absent from this list.
                | MigrationError::NoSchema(_)
                | MigrationError::Failed { .. }
                | MigrationError::AirhouseRule { .. }
        )
    }

    /// Whether re-running the same publish could succeed.
    ///
    /// `Infra` joins `Busy` here: a dropped connection or an unreachable store
    /// is transient by nature, and the same bundle republished after it clears
    /// will apply exactly the files it was going to apply.
    pub fn is_retryable(&self) -> bool {
        matches!(self, MigrationError::Busy | MigrationError::Infra { .. })
    }
}

/// One `.sql` file the bundle declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeclaredMigration {
    /// Path RELATIVE to the declared directory — the ledger key. Relative so
    /// renaming the directory in `oxy-app.json` does not orphan the ledger and
    /// re-run every file against tables that already exist.
    pub filename: String,
    /// Lowercase hex SHA-256 of the bytes as shipped.
    pub checksum: String,
    pub sql: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Applied {
    pub applied: Vec<String>,
    pub already_applied: usize,
}

impl Applied {
    /// One line for the publish log. Empty when the app declares nothing, so a
    /// caller can log it unconditionally.
    pub(crate) fn summary(&self) -> String {
        if self.applied.is_empty() && self.already_applied == 0 {
            return String::new();
        }
        format!(
            "{} schema migration(s) applied, {} already present",
            self.applied.len(),
            self.already_applied
        )
    }
}
