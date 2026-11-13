use super::*;

/// What `sequence` is handed as the checks: records that it ran.
async fn checks(ran: &std::sync::atomic::AtomicBool) -> Tick {
    ran.store(true, Ordering::SeqCst);
    Tick {
        checked: 1,
        ..Default::default()
    }
}

/// A default-branch pass that stopped (GitHub unwell, or its budget spent)
/// does not skip the checks of workspaces that already have a branch.
#[tokio::test]
async fn a_stopped_default_branch_pass_does_not_skip_the_checks() {
    let ran = std::sync::atomic::AtomicBool::new(false);
    let stopped = async {
        default_branch::Pass {
            stopped: true,
            ..Default::default()
        }
    };
    let tick = sequence(stopped, checks(&ran)).await;
    assert!(ran.load(Ordering::SeqCst), "the check loop ran");
    assert_eq!(tick.checked, 1);
    assert!(!tick.rate_limited);
}

/// A rate limit in the pass pauses this process as the check loop does (the
/// loop itself still runs, and asks nothing while paused).
#[tokio::test]
async fn a_rate_limited_default_branch_pass_pauses_the_process() {
    let ran = std::sync::atomic::AtomicBool::new(false);
    let limited = async {
        default_branch::Pass {
            stopped: true,
            rate_limited: true,
            ..Default::default()
        }
    };
    let tick = sequence(limited, checks(&ran)).await;
    let was_paused = paused();
    PAUSED_UNTIL.store(0, Ordering::Relaxed);
    assert!(was_paused);
    assert!(ran.load(Ordering::SeqCst));
    assert!(tick.rate_limited);
}
