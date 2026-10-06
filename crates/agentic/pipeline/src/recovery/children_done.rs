//! A parent whose children have all reported, and what continues it.
//!
//! The last child of a delegation writes its outcome before its parent is
//! resumed. A driver that dies in between leaves the parent parked at the
//! delegation with every answer already on record, and recovery then has two
//! ways to move it. The coordinator it rebuilds finds the parent and resumes
//! it with the aggregated answer ([`PendingResume`]). The tree walk re-launches
//! a task that has a checkpoint. Doing both continued the run twice: for an
//! analytics or builder parent a second pipeline, started with an empty
//! answer; for an automation a second decision, which saw the step as not yet
//! run and delegated it again.
//!
//! [`settle`] picks one per parent before the walk, and before any worker
//! exists.

use std::collections::HashSet;

use agentic_runtime::coordinator::PendingResume;

use super::root_entry::RootEntry;

/// What continues one parent whose children have all reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Continuation {
    /// The coordinator assigns the resume that carries the children's answer.
    /// The walk leaves the parent alone.
    Coordinator,
    /// That resume was assigned before the driver died, and it is the parent's
    /// queue entry now ([`RootEntry::Continues`]). The worker runs it. Neither
    /// the walk nor the coordinator resumes the parent again.
    QueueEntry,
    /// There is no checkpoint, so no resume can be built and nothing can carry
    /// the answer. The parent stays the walk's, which treats it as it does any
    /// other task that cannot be resumed.
    Walk,
}

/// Pure, so the mapping — the whole of "is this parent continued twice" — is
/// assertable without a database.
pub(super) fn classify(has_checkpoint: bool, entry_is_the_resume: bool) -> Continuation {
    match (has_checkpoint, entry_is_the_resume) {
        (false, _) => Continuation::Walk,
        (true, true) => Continuation::QueueEntry,
        (true, false) => Continuation::Coordinator,
    }
}

/// The verdict for every parent in one run's tree whose children are all in.
#[derive(Debug, Default)]
pub(super) struct ChildrenDone {
    /// Continued by a resume that carries the children's answer.
    resumed: HashSet<String>,
    /// Every child in, and no checkpoint to resume from.
    unresumable: HashSet<String>,
    /// What the coordinator is handed: the resumes it assigns, and — marked
    /// [`PendingResume::already_assigned`] — the ones it only records as
    /// running because the queue already holds them.
    pub(super) for_coordinator: Vec<PendingResume>,
}

impl ChildrenDone {
    /// A resume carrying its children's answer continues this task, so the
    /// walk must not re-launch it.
    pub(super) fn resumes(&self, task_id: &str) -> bool {
        self.resumed.contains(task_id)
    }

    /// Every child of this task has reported and it has no checkpoint.
    pub(super) fn cannot_resume(&self, task_id: &str) -> bool {
        self.unresumable.contains(task_id)
    }
}

/// Decide, once, what continues each parent the rebuilt coordinator reports as
/// having every child in.
///
/// `root_entry` is the root's own verdict from [`super::root_entry::reconcile`].
/// Only the root's entry is read there, so only the root can be continued by
/// its queue entry; any other parent is the coordinator's.
pub(super) fn settle(
    root_id: &str,
    root_entry: RootEntry,
    pending: Vec<PendingResume>,
) -> ChildrenDone {
    let mut done = ChildrenDone::default();
    for mut resume in pending {
        let entry_is_the_resume =
            resume.parent_task_id == root_id && root_entry == RootEntry::Continues;
        let continuation = classify(resume.has_checkpoint, entry_is_the_resume);
        if continuation == Continuation::Walk {
            done.unresumable.insert(resume.parent_task_id);
            continue;
        }
        resume.already_assigned = continuation == Continuation::QueueEntry;
        done.resumed.insert(resume.parent_task_id.clone());
        done.for_coordinator.push(resume);
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(task_id: &str, has_checkpoint: bool) -> PendingResume {
        PendingResume {
            parent_task_id: task_id.into(),
            answer: "store 7 sold the most".into(),
            has_checkpoint,
            already_assigned: false,
        }
    }

    /// What the coordinator is handed, as `(task, already assigned)`.
    fn handed(done: &ChildrenDone) -> Vec<(&str, bool)> {
        done.for_coordinator
            .iter()
            .map(|r| (r.parent_task_id.as_str(), r.already_assigned))
            .collect()
    }

    /// The reported case: nothing on the queue describes the resume yet, so
    /// the coordinator assigns it and the walk keeps its hands off.
    #[test]
    fn a_parent_with_a_checkpoint_is_the_coordinators() {
        assert_eq!(classify(true, false), Continuation::Coordinator);
        let done = settle("root", RootEntry::Unchanged, vec![pending("root", true)]);
        assert!(done.resumes("root"));
        assert_eq!(handed(&done), vec![("root", false)]);
    }

    /// A stale entry was taken out of `queued` by `root_entry`; it is not the
    /// resume, so the coordinator still assigns one.
    #[test]
    fn a_stale_root_entry_does_not_stand_in_for_the_resume() {
        let done = settle("root", RootEntry::Stale, vec![pending("root", true)]);
        assert!(done.resumes("root"));
        assert_eq!(handed(&done), vec![("root", false)]);
    }

    #[test]
    fn a_root_whose_entry_is_the_resume_is_not_assigned_again() {
        assert_eq!(classify(true, true), Continuation::QueueEntry);
        let done = settle("root", RootEntry::Continues, vec![pending("root", true)]);
        assert!(done.resumes("root"), "the walk still leaves it alone");
        assert_eq!(
            handed(&done),
            vec![("root", true)],
            "the coordinator is told, not asked to assign"
        );
    }

    /// `root_entry` describes the root's queue entry and nobody else's.
    #[test]
    fn only_the_root_is_continued_by_the_roots_entry() {
        let done = settle(
            "root",
            RootEntry::Continues,
            vec![pending("root.1", true), pending("root", true)],
        );
        assert_eq!(handed(&done), vec![("root.1", false), ("root", true)]);
        assert!(done.resumes("root.1"));
    }

    /// No checkpoint, no resume: the coordinator could only log and leave the
    /// parent waiting, so it is not counted on and the walk keeps the task.
    #[test]
    fn a_parent_without_a_checkpoint_stays_the_walks() {
        assert_eq!(classify(false, false), Continuation::Walk);
        assert_eq!(classify(false, true), Continuation::Walk);
        let done = settle("root", RootEntry::Unchanged, vec![pending("root", false)]);
        assert!(!done.resumes("root"));
        assert!(done.cannot_resume("root"));
        assert!(done.for_coordinator.is_empty());
    }

    #[test]
    fn a_task_nobody_reported_is_left_to_the_walk() {
        let done = settle("root", RootEntry::Unchanged, vec![]);
        assert!(!done.resumes("root"));
        assert!(!done.cannot_resume("root"));
    }
}
