//! Which slot a checkout takes. Pure, so the policy is testable without
//! connectors.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SlotState {
    /// Open and live, and no caller holds it.
    Idle,
    /// Open and live, and a caller still holds it.
    InUse,
    /// Empty, or its connection died: taking it opens a new connection.
    Vacant,
}

/// The slot to take, given each slot's state (`None`: another checkout holds
/// its lock). An idle open connection first, so a sequential caller keeps
/// reusing one connection; then an empty slot, so the pool grows only with
/// concurrent callers. `None` when neither exists, and the caller shares a
/// connection that is in use.
pub(super) fn choose_slot(states: &[Option<SlotState>]) -> Option<usize> {
    let first = |want| states.iter().position(|s| *s == Some(want));
    first(SlotState::Idle).or_else(|| first(SlotState::Vacant))
}
