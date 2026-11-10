//! The `oxy migrate-agents` report: JSON for the census, text for a person.

use serde::Serialize;

use ::oxy::theme::StyledText;

use super::refs::{BlockedRef, RefRewrite};

/// Everything one run found and did (or, with `--dry-run`, would do). The
/// `--json` shape is a contract: the retirement memo's gate reads it.
#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub dry_run: bool,
    /// Every classic `*.agent.yml` the walker's rules discover.
    pub classic_files: Vec<String>,
    pub translated: Vec<Translated>,
    /// Files left in place: a field with no agentic equivalent (refused).
    pub unmappable: Vec<Unmappable>,
    pub refs_rewritten: Vec<RefRewrite>,
    /// References to a classic file that was not translated. They stay broken.
    pub refs_blocked: Vec<BlockedRef>,
    /// Files left in place: their target exists or their agent id is taken.
    pub collisions: Vec<Collision>,
}

#[derive(Debug, Serialize)]
pub struct Translated {
    pub from: String,
    pub to: String,
    /// Fields with no agentic equivalent whose behaviour the pipeline owns.
    pub dropped: Vec<String>,
    /// Refused fields dropped anyway because of `--force`.
    pub forced: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Unmappable {
    pub path: String,
    pub fields: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Collision {
    pub path: String,
    pub target: String,
    pub reason: String,
}

impl Report {
    pub fn print_text(&self) {
        if self.classic_files.is_empty() {
            println!(
                "{}",
                "No .agent.yml files found — nothing to migrate.".text()
            );
            return;
        }
        let verb = if self.dry_run {
            "would translate"
        } else {
            "translated"
        };
        for t in &self.translated {
            println!("{}", format!("{verb} {} -> {}", t.from, t.to).text());
            print_list("dropped", &t.dropped, false);
            print_list("forced (dropped)", &t.forced, true);
            print_list("note", &t.notes, true);
        }
        for u in &self.unmappable {
            let fields = u.fields.join(", ");
            println!(
                "{}",
                format!("cannot be mapped: {} ({fields})", u.path).warning()
            );
            print_list("note", &u.notes, true);
        }
        for c in &self.collisions {
            let line = format!(
                "collision, skipped: {} -> {} ({})",
                c.path, c.target, c.reason
            );
            println!("{}", line.warning());
        }
        let verb = if self.dry_run {
            "would rewrite"
        } else {
            "rewrote"
        };
        for r in &self.refs_rewritten {
            println!(
                "{}",
                format!("{verb} {}:{} {} -> {}", r.file, r.line, r.from, r.to).text()
            );
        }
        for b in &self.refs_blocked {
            let line = format!(
                "still broken: {}:{} {} ({})",
                b.file, b.line, b.value, b.reason
            );
            println!("{}", line.warning());
        }
        self.print_summary();
    }

    fn print_summary(&self) {
        let summary = format!(
            "\n{} classic file(s): {} translated, {} cannot be mapped, {} collided; \
             {} reference(s) rewritten, {} left broken.",
            self.classic_files.len(),
            self.translated.len(),
            self.unmappable.len(),
            self.collisions.len(),
            self.refs_rewritten.len(),
            self.refs_blocked.len(),
        );
        if self.dry_run {
            println!("{}", summary.tertiary());
            println!("{}", "Dry run: nothing was written.".tertiary());
        } else {
            println!("{}", summary.success());
        }
        if !self.unmappable.is_empty() && !self.dry_run {
            let hint = "Files that cannot be mapped were left in place. Rewrite them by hand, \
                        or re-run with --force to drop the refused fields.";
            println!("{}", hint.warning());
        }
    }
}

fn print_list(label: &str, items: &[String], warn: bool) {
    for item in items {
        let line = format!("    {label}: {item}");
        if warn {
            println!("{}", line.warning());
        } else {
            println!("{}", line.tertiary());
        }
    }
}
