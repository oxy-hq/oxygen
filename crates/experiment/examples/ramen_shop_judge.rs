//! cargo run -p oxy-experiment --example ramen_shop_judge -- <dir>
//!
//! <dir> holds `truth.json` and the four `GET /experiments/{name}/result` bodies
//! (`staggered.json`, `grouped.json`, `switchback.json`, `null.json`), as written by
//! `scripts/eval/ramen_shop/collect.sh`. Prints one line per check and exits 1 on any
//! MISS. There is no retry flag and there never will be: a miss is examined.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::ExitCode;

use oxy_experiment::eval::*;
use oxy_experiment::report::ReportView;
use serde::Deserialize;

#[derive(Deserialize)]
struct Truth {
    planted: Planted,
    non_responder: String,
    scopes: BTreeMap<String, Scope>,
}

fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> T {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn report(dir: &Path, label: &str) -> ReportView {
    #[derive(Deserialize)]
    struct Body {
        report: ReportView,
    }
    read::<Body>(&dir.join(format!("{label}.json"))).report
}

fn main() -> ExitCode {
    let dir = std::env::args()
        .nth(1)
        .expect("usage: ramen_shop_judge <dir>");
    let dir = Path::new(&dir);
    let truth: Truth = read(&dir.join("truth.json"));
    let mut checks: Vec<(String, Check)> = Vec::new();
    for label in ["staggered", "grouped", "switchback"] {
        let (r, scope) = (report(dir, label), &truth.scopes[label]);
        match truth.planted.truth(scope) {
            Some(t) => checks.push((
                label.into(),
                interval_covers("interval_covers_planted", &r, t),
            )),
            None => checks.push((
                label.into(),
                Check {
                    name: "truth",
                    verdict: Verdict::Inconclusive,
                    detail: "no planted truth for this scope".into(),
                },
            )),
        }
        checks.push((label.into(), slope_covered(&r, &truth.planted)));
        checks.push((
            label.into(),
            prior_verdict_matches_truth(&r, &truth.planted),
        ));
        if let Scope::Units { treated } = scope {
            checks.push((
                label.into(),
                non_responder_is_itt(&r, &truth.planted, treated, &truth.non_responder),
            ));
        }
    }
    checks.push(("null".into(), claims_no_effect(&report(dir, "null"))));
    let mut missed = false;
    for (label, c) in &checks {
        missed |= c.verdict == Verdict::Miss;
        println!(
            "{:<11} {:<24} {:<13} {}",
            label,
            c.name,
            format!("{:?}", c.verdict).to_uppercase(),
            c.detail
        );
    }
    if missed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
