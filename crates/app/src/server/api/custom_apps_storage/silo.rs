//! Which silo a `ctx.storage` call works in — one per app **environment**
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §4.2).
//!
//! Production's silo is `customer-app-storage/<app_id>/`, exactly as before
//! environments existed. Every other environment of the app gets a
//! **sibling**, `customer-app-storage/<app_id>~<env>/`:
//!
//! - writes — `getUploadUrl`, `put`, `delete`, and `copy`'s destination — land
//!   in the environment's own silo, so staging can never overwrite or delete a
//!   production object;
//! - reads — `get`, `head`, `getDownloadUrl`, and `copy`'s source — look there
//!   first and then at production's **same relative key**, read-only, so
//!   staging sees production's files without a copy step;
//! - `list` lists the environment's silo alone and never merges, so a listing
//!   is always of things this environment can also delete.
//!
//! **A sibling, not a sub-prefix of production's** (`<id>/staging/…`):
//! production's own `list`, its metering walk and its app-deletion sweep all
//! work under `<id>/`, and a nested silo would put staging's objects inside
//! every one of them. `~` survives neither a UUID nor a sanitized pathname
//! segment (`sanitize_segment` maps it to `_`), so no key one silo can name
//! lands in the other. Both sit under `customer-app-storage/`, so the bucket's
//! tag-based lifecycle rules (`retention`) expire either alike.
//!
//! **A caller's key may name either silo.** A staging function often holds a
//! production key — a row in `ctx.oltp` naming an upload. Outside production
//! such a key is re-rooted into the environment's silo for a write or delete,
//! and answered through the fallback for a read; a key of another app is
//! refused in every environment.

use oxy_app_core::custom_app_environment::AppEnvironment;
use uuid::Uuid;

/// Every silo of every app lives under this root.
pub(crate) const SILO_ROOT: &str = "customer-app-storage/";

/// One app environment's asset silo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Silo {
    app_id: Uuid,
    /// `None` for production; otherwise the environment's name (`staging`,
    /// `dev-<handle>`), as `AppEnvironment::name` spells it.
    environment: Option<String>,
}

impl Silo {
    /// Production's silo: `customer-app-storage/<app_id>/`.
    pub fn production(app_id: Uuid) -> Self {
        Self {
            app_id,
            environment: None,
        }
    }

    /// The silo `environment` of the app works in.
    pub fn for_environment(app_id: Uuid, environment: &AppEnvironment) -> Self {
        match environment {
            AppEnvironment::Production => Self::production(app_id),
            other => Self {
                app_id,
                environment: Some(other.name()),
            },
        }
    }

    pub fn app_id(&self) -> Uuid {
        self.app_id
    }

    pub fn is_production(&self) -> bool {
        self.environment.is_none()
    }

    /// The key prefix of this silo, with its trailing slash.
    pub fn prefix(&self) -> String {
        match &self.environment {
            None => format!("{SILO_ROOT}{}/", self.app_id),
            Some(env) => format!("{SILO_ROOT}{}~{env}/", self.app_id),
        }
    }

    /// Where a read that misses here falls back to: production's silo of the
    /// same app, read-only. `None` in production.
    pub fn fallback(&self) -> Option<Silo> {
        (!self.is_production()).then(|| Silo::production(self.app_id))
    }

    /// The app-relative part of `key` when it names an object of this silo —
    /// or, outside production, of production's, which this silo re-roots.
    /// `None` for a key of any other silo or app.
    pub(super) fn relative<'k>(&self, key: &'k str) -> Option<&'k str> {
        key.strip_prefix(self.prefix().as_str()).or_else(|| {
            self.fallback()
                .and_then(|production| key.strip_prefix(production.prefix().as_str()))
        })
    }

    /// `relative` as a key of this silo.
    pub(super) fn key(&self, relative: &str) -> String {
        format!("{}{relative}", self.prefix())
    }
}

/// Which silo of `app_id` a stored `key` belongs to, and its app-relative part:
/// `Some((None, rel))` for production, `Some((Some(env), rel))` for an
/// environment's, `None` for a key of no silo of this app. The metering walk
/// reads every silo of an app and has to tell them apart.
pub(crate) fn split_silo_key(app_id: Uuid, key: &str) -> Option<(Option<&str>, &str)> {
    let rest = key.strip_prefix(SILO_ROOT)?;
    let id = app_id.to_string();
    let rest = rest.strip_prefix(id.as_str())?;
    if let Some(relative) = rest.strip_prefix('/') {
        return Some((None, relative));
    }
    let (env, relative) = rest.strip_prefix('~')?.split_once('/')?;
    (!env.is_empty()).then_some((Some(env), relative))
}

/// The raw prefix every non-production silo of `app_id` starts with,
/// `customer-app-storage/<app_id>~`. A string prefix, not a directory: the
/// object store lists and deletes by it directly.
pub(crate) fn environment_silos_prefix(app_id: Uuid) -> String {
    format!("{SILO_ROOT}{app_id}~")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> Uuid {
        Uuid::from_u128(7)
    }

    fn staging() -> Silo {
        Silo::for_environment(app(), &AppEnvironment::Staging)
    }

    #[test]
    fn production_keeps_the_silo_it_always_had() {
        let silo = Silo::for_environment(app(), &AppEnvironment::Production);
        assert_eq!(silo, Silo::production(app()));
        assert_eq!(silo.prefix(), format!("customer-app-storage/{}/", app()));
        assert_eq!(silo.fallback(), None);
    }

    #[test]
    fn an_environment_gets_a_sibling_that_production_cannot_name() {
        let stg = staging();
        assert_eq!(
            stg.prefix(),
            format!("customer-app-storage/{}~staging/", app())
        );
        let production = Silo::production(app()).prefix();
        assert!(
            !stg.prefix().starts_with(production.as_str()),
            "a sibling, never inside production's silo"
        );
        let dev = Silo::for_environment(
            app(),
            &AppEnvironment::Dev {
                handle: "luong".into(),
            },
        );
        assert_eq!(
            dev.prefix(),
            format!("customer-app-storage/{}~dev-luong/", app())
        );
        assert_eq!(stg.fallback(), Some(Silo::production(app())));
    }

    #[test]
    fn a_production_key_is_re_rooted_outside_production_and_nowhere_else() {
        let production_key = format!("customer-app-storage/{}/uploads/a.pdf", app());
        assert_eq!(staging().relative(&production_key), Some("uploads/a.pdf"));
        let staging_key = staging().key("uploads/a.pdf");
        assert_eq!(
            Silo::production(app()).relative(&staging_key),
            None,
            "production never reads or writes a staging key"
        );
        let other = format!("customer-app-storage/{}/x", Uuid::from_u128(8));
        assert_eq!(staging().relative(&other), None);
    }

    #[test]
    fn split_tells_every_silo_of_the_app_apart() {
        let a = app();
        assert_eq!(
            split_silo_key(a, &format!("customer-app-storage/{a}/uploads/x.png")),
            Some((None, "uploads/x.png"))
        );
        assert_eq!(
            split_silo_key(
                a,
                &format!("customer-app-storage/{a}~staging/generated/r.pdf")
            ),
            Some((Some("staging"), "generated/r.pdf"))
        );
        assert_eq!(
            split_silo_key(a, &format!("customer-app-storage/{a}~/x")),
            None
        );
        assert_eq!(
            split_silo_key(a, &format!("customer-app-storage/{}/x", Uuid::from_u128(8))),
            None
        );
        assert!(
            staging()
                .prefix()
                .starts_with(environment_silos_prefix(a).as_str())
        );
        assert!(
            !Silo::production(a)
                .prefix()
                .starts_with(environment_silos_prefix(a).as_str()),
            "the environments' raw prefix never reaches production's silo"
        );
    }
}
