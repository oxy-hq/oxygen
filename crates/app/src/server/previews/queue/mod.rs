//! The half of previews' run queue that code below the previews context calls:
//! the queue step ([`run::advance`]) and the flag that gates it, the run
//! lifecycle ([`lifecycle`]: when a run counts as over, cancelling a deleted
//! preview's queued runs), queueing a revision's change check
//! ([`analyze::enqueue_after_staging_compile`], from the compile worker) and
//! seeding an Airway sample's run ([`sample_seed`], from the queue step), and
//! the per-pass [`sweep`] the global driver runs (with its [`ceiling`] and the
//! compares it queues, [`compare`]).
//!
//! Rows only, over the registry's own tables: nothing here runs a check, a
//! sample or a procedure. Its callers outside previews are the compile worker,
//! the custom-app automation run's preview mode and its settle, so it stays in
//! `oxy-app` with the registry, the DDL and the pin when the rest of previews
//! (the handlers, the executors, maintenance) moves to its own surface crate.
//! Nothing here may name those: `queue_names_no_upper_module` holds it.

pub mod analyze;
pub mod ceiling;
pub mod compare;
pub mod lifecycle;
pub mod run;
pub mod sample_seed;
pub mod sweep;

#[cfg(test)]
mod tests {
    /// The modules that leave `oxy-app` with the previews surface. A file here
    /// naming one would pull it back below, or break when it goes.
    const UPPER: &[&str] = &[
        "previews::analyze::",
        "previews::compare",
        "previews::checks",
        "previews::drop",
        "previews::maintenance",
        "previews::runs::",
        "previews::sample::",
        "previews::service",
        "previews::revisions",
        "super::super::analyze",
        "super::super::runs",
        "super::super::sample",
    ];

    #[test]
    fn queue_names_no_upper_module() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/server/previews/queue");
        let mut scanned = 0;
        for entry in std::fs::read_dir(&dir).expect("queue dir") {
            let path = entry.expect("entry").path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            scanned += 1;
            let text = std::fs::read_to_string(&path).expect("read");
            // Production code only: this file's own test names every upper
            // module on purpose.
            let text = text.split("#[cfg(test)]").next().unwrap_or(&text);
            for (n, line) in text.lines().enumerate() {
                let code = line.split("//").next().unwrap_or(line);
                for upper in UPPER {
                    assert!(
                        !code.contains(upper),
                        "{}:{} names `{upper}`, which leaves `oxy-app` with the previews \
                         surface; the queue must not depend on it",
                        path.display(),
                        n + 1
                    );
                }
            }
        }
        assert!(
            scanned >= 5,
            "scanned only {scanned} files under {}",
            dir.display()
        );
    }
}
