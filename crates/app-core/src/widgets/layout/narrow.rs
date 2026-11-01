//! Narrowing what the home draws for one person: the copies a layout's `for`
//! leaves them out of (`visible`), and any other copies to leave out (`without`).
//! Both run on `resolve`'s output, before a single widget function is called.

use super::{Audience, Drawn, Placed, Viewer};

/// `drawn`, as `viewer` sees it: a copy is kept when its group's `for` and its
/// own `for` both admit them. A copy the layout does not list, added to a group
/// that has a `for`, is `ungated`: the group's does not reach it, so it stays. A group the filter empties is
/// omitted, so the home never draws a title over nothing.
pub fn visible(drawn: Vec<Drawn>, viewer: &Viewer) -> Vec<Drawn> {
    narrow(drawn, |group, copy| {
        let group_admits = copy.ungated || admits(group, viewer);
        group_admits && admits(copy.audience.as_ref(), viewer)
    })
}

/// `drawn` without the copies `leave_out` names; a group it empties is omitted.
pub fn without(drawn: Vec<Drawn>, mut leave_out: impl FnMut(&Placed) -> bool) -> Vec<Drawn> {
    narrow(drawn, |_, copy| !leave_out(copy))
}

fn admits(audience: Option<&Audience>, viewer: &Viewer) -> bool {
    audience.is_none_or(|a| a.admits(viewer))
}

fn narrow(
    drawn: Vec<Drawn>,
    mut keep: impl FnMut(Option<&Audience>, &Placed) -> bool,
) -> Vec<Drawn> {
    drawn
        .into_iter()
        .filter_map(|d| match d {
            Drawn::Widgets {
                title,
                mut placed,
                hide_others,
                audience,
            } => {
                let before = placed.len();
                placed.retain(|copy| keep(audience.as_ref(), copy));
                (before == 0 || !placed.is_empty()).then_some(Drawn::Widgets {
                    title,
                    placed,
                    hide_others,
                    audience,
                })
            }
            other => Some(other),
        })
        .collect()
}
