//! What a platform-standing read answers. Moved here from
//! `oxy_server_authz::globals`, which re-exports both, so the port and the one
//! reader of the platform sources name the same types.

use oxy_authz::PlatformStanding as Grant;

/// What Oxy's platform sources say about a person, **as flags to display**. Not a
/// decision, and deliberately lossy: it says *that* someone is staff, never what they
/// may do. Feed the grant (`globals::platform_grant_checked`, or [`Checked::grant`]) to
/// a ring when you need the latter.
///
/// Renamed off `PlatformStanding` when platform standing became a real grant — that
/// name now belongs to [`oxy_authz::PlatformStanding`], which carries the capabilities
/// and scope. Two types with one name, one of them a boolean pair, is how a call site
/// ends up deciding access from a display flag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlatformFlags {
    /// In the `OXY_OWNER` env allow-list.
    pub is_global_owner: bool,
    /// Holds a row in the platform-grant table (`app_admins`) — **any** role. A
    /// display flag only: an App Operator and a Global Admin both report `true` here,
    /// which is precisely why nothing may authorize from it.
    pub is_global_admin: bool,
}

impl PlatformFlags {
    /// Either flag — "is this Oxy staff at all". The `oxy_owner_or_app_admin` shape.
    pub fn is_staff(self) -> bool {
        self.is_global_owner || self.is_global_admin
    }
}

/// A resolved platform read: the display flags **and** the grant behind them.
///
/// They travel together because they are one database read and must never disagree.
/// The loader takes [`Self::grant`] (what may this person do); `/me`-style payloads take
/// [`Self::flags`] (is this person staff). A call site that authorizes from `flags` has
/// re-created the boolean this whole change removed — take the grant.
#[derive(Clone, Debug)]
pub struct Checked {
    pub flags: PlatformFlags,
    pub grant: Option<Grant>,
}
