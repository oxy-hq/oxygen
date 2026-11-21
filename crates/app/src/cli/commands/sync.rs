//! `oxy sync` — a one-release stub. The command is `oxyc sync` now
//! (`@oxy-hq/cli`, `sdk/cli`): the server introspects the warehouse and renders
//! the `.databases/` files (`POST /api/{workspace_id}/databases/sync`,
//! `oxy::semantic::render`), and `oxyc` writes them into the checkout. The
//! laptop no longer needs the warehouse credentials, only an `oxyc` login.
//!
//! Customers run this from scripts, so it is not deleted outright: the args
//! still parse, so an old invocation lands here and is told the exact `oxyc`
//! command to type, and the process exits `2` (a usage error). Delete this file
//! and its `SubCommand` arm one release after the `oxyc` verb ships
//! (`internal-docs/oxy-cli-to-oxyc.md`, step 12).

use clap::Parser;

#[derive(Parser, Debug)]
pub struct SyncArgs {
    /// Specific database to sync (syncs all if not specified)
    ///
    /// Target a single database connection from your config.yml
    /// instead of syncing metadata from all configured databases.
    pub database: Option<String>,
    /// Specific datasets/tables to sync within the database
    ///
    /// Limit synchronization to particular tables or schemas
    /// instead of processing the entire database structure.
    #[clap(long, short = 'd', num_args = 0..)]
    pub datasets: Vec<String>,
    /// Overwrite existing metadata files during sync
    ///
    /// Replace existing schema files and metadata instead of
    /// skipping tables that have already been synchronized.
    #[clap(
        long,
        short = 'o',
        default_value_t = false,
        help = "Overwrite existing files during sync"
    )]
    pub overwrite: bool,
}

/// The exit code the stub ends with: a usage error, as for an unknown command.
pub const MOVED_EXIT_CODE: i32 = 2;

/// What the stub prints: the `oxyc` invocation equivalent to `args`. The
/// workspace is a deployed one now, so the caller fills in its id.
pub fn moved_message(args: &SyncArgs) -> String {
    let mut command = String::from("oxyc sync");
    if let Some(database) = &args.database {
        command.push(' ');
        command.push_str(&shell_quote(database));
    }
    for dataset in &args.datasets {
        command.push_str(" -d ");
        command.push_str(&shell_quote(dataset));
    }
    if args.overwrite {
        command.push_str(" --overwrite");
    }
    command.push_str(" --workspace <workspace-id>");
    format!(
        "`oxy sync` has moved to oxyc. From the root of your checkout, run:\n\n    {command}\n\n\
         The server introspects the warehouses the DEPLOYED config.yml names, so push a \
         config change before syncing it. Install oxyc with `npm i -g @oxy-hq/cli` \
         (or run `npx @oxy-hq/cli sync`)."
    )
}

/// Single-quote `s` for a POSIX shell when it holds anything but safe characters.
fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@+=,".contains(c));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Parser, Debug)]
    struct Probe {
        #[clap(flatten)]
        args: SyncArgs,
    }

    fn parse(argv: &[&str]) -> SyncArgs {
        Probe::try_parse_from(std::iter::once("oxy").chain(argv.iter().copied()))
            .expect("an old invocation must still parse")
            .args
    }

    /// The invocations `docs/build/connect-data/sync-schema.mdx` and the old
    /// integration tests (`crates/core/tests/integration/sync.rs`) used.
    #[test]
    fn old_invocations_still_parse() {
        let a = parse(&[]);
        assert_eq!(
            (a.database, a.datasets.len(), a.overwrite),
            (None, 0, false)
        );
        let a = parse(&["primary_database", "-d", "dbt_prod_metrics", "--overwrite"]);
        assert_eq!(a.database.as_deref(), Some("primary_database"));
        assert_eq!(a.datasets, ["dbt_prod_metrics"]);
        assert!(a.overwrite);
        let a = parse(&["my_database", "-d", "customers", "-d", "orders", "-o"]);
        assert_eq!(a.datasets, ["customers", "orders"]);
        assert!(a.overwrite);
    }

    #[test]
    fn prints_the_equivalent_oxyc_command() {
        assert!(
            moved_message(&parse(&[])).contains("\n    oxyc sync --workspace <workspace-id>\n")
        );
        assert!(
            moved_message(&parse(&["local", "--overwrite"]))
                .contains("\n    oxyc sync local --overwrite --workspace <workspace-id>\n")
        );
        assert!(
            moved_message(&parse(&["bq", "-d", "a", "-d", "my set"]))
                .contains("\n    oxyc sync bq -d a -d 'my set' --workspace <workspace-id>\n")
        );
    }

    #[test]
    fn exits_as_a_usage_error() {
        assert_eq!(MOVED_EXIT_CODE, 2);
    }
}
