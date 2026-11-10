//! The narrowing every inbox route shares.
//!
//! Always the person's own signals, in the orgs the request's token reaches.
//! Then, each optional and each narrowing: one org (`org_id`), one app
//! (`app_id`), one source (`source=oxygen|<app id>`), a category subtree of
//! that source (`category=<path>`, which needs `source`), one kind and one
//! thread. The list, the badge and `read-all` take the same parameters, so
//! "mark all read" clears exactly what the page shows.

use axum::http::StatusCode;
use entity::{apps, notifications};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::Deserialize;
use uuid::Uuid;

use super::catalog::model::{GENERAL, valid_path};
use super::catalog::platform::oxygen;
use super::catalog::source::{KindSet, SourceCatalogue};
use super::input::valid_kind;

/// Longest `thread` filter: a thread key is 32 hex characters.
const MAX_THREAD_FILTER: usize = 64;

/// The filter as the query string carries it. Flattened into each route's
/// own query struct by hand: `serde(flatten)` cannot read a `bool` or a
/// number out of a query string.
///
/// Unknown parameters are refused (`400`), not ignored: `read-all` is a write
/// taking this, and a filter it did not understand would clear everything.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilterParams {
    pub org_id: Option<Uuid>,
    /// Platform notices have no app, so they never match one.
    pub app_id: Option<Uuid>,
    /// `oxygen`, or an app's id.
    pub source: Option<String>,
    /// A category path of `source`, covering its subtree.
    pub category: Option<String>,
    pub kind: Option<String>,
    /// An item's opaque `thread`.
    pub thread: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceSel {
    Oxygen,
    App(Uuid),
}

/// A filter checked and resolved against its source's catalogue.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    org_id: Option<Uuid>,
    app_id: Option<Uuid>,
    source: Option<SourceSel>,
    kinds: Option<KindSet>,
    kind: Option<String>,
    thread: Option<String>,
}

impl FilterParams {
    /// `400` for a malformed parameter or a `category` without a `source`.
    /// A well-formed path the source does not have matches nothing, the way
    /// any filter does.
    pub async fn resolve(&self, db: &DatabaseConnection) -> Result<Filter, StatusCode> {
        let source = match self.source.as_deref() {
            None => None,
            Some("oxygen") => Some(SourceSel::Oxygen),
            Some(id) => Some(SourceSel::App(
                Uuid::parse_str(id).map_err(|_| StatusCode::BAD_REQUEST)?,
            )),
        };
        if self.kind.as_deref().is_some_and(|k| !valid_kind(k))
            || self
                .thread
                .as_deref()
                .is_some_and(|t| t.is_empty() || t.len() > MAX_THREAD_FILTER)
        {
            return Err(StatusCode::BAD_REQUEST);
        }
        let kinds = match (self.category.as_deref(), source) {
            (None, _) => None,
            (Some(_), None) => return Err(StatusCode::BAD_REQUEST),
            (Some(path), Some(source)) => {
                if !(path == GENERAL || valid_path(path)) {
                    return Err(StatusCode::BAD_REQUEST);
                }
                let catalogue = catalogue_of(db, source)
                    .await
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                Some(catalogue.kinds_in_scope(path))
            }
        };
        Ok(Filter {
            org_id: self.org_id,
            app_id: self.app_id,
            source,
            kinds,
            kind: self.kind.clone(),
            thread: self.thread.clone(),
        })
    }
}

async fn catalogue_of(
    db: &DatabaseConnection,
    source: SourceSel,
) -> Result<SourceCatalogue, sea_orm::DbErr> {
    match source {
        SourceSel::Oxygen => Ok(oxygen()),
        SourceSel::App(id) => match apps::Entity::find_by_id(id).one(db).await? {
            Some(app) => super::catalog::live::load(db, &app).await,
            // No such app: nothing of it is in anybody's inbox either.
            None => Ok(super::catalog::live::from_manifest(None)),
        },
    }
}

impl Filter {
    /// `find` narrowed to `user`'s signals within this filter. An org that
    /// ended the request's token's reach (API-tokens design §5) is left out,
    /// including when `org_id` names it.
    pub fn apply<Q: QueryFilter>(&self, find: Q, user: &oxy_auth::types::AuthenticatedUser) -> Q {
        use notifications::Column as C;
        let mut find = find.filter(C::UserId.eq(user.id));
        if let Some(org) = self.org_id {
            find = find.filter(C::OrgId.eq(org));
        }
        if let Some(app) = self.app_id {
            find = find.filter(C::AppId.eq(app));
        }
        find = match self.source {
            None => find,
            Some(SourceSel::Oxygen) => find.filter(C::AppId.is_null()),
            Some(SourceSel::App(app)) => find.filter(C::AppId.eq(app)),
        };
        find = match &self.kinds {
            None => find,
            Some(KindSet::Only(kinds)) => find.filter(C::Kind.is_in(kinds.clone())),
            Some(KindSet::AllBut(kinds)) => find.filter(C::Kind.is_not_in(kinds.clone())),
        };
        if let Some(kind) = &self.kind {
            find = find.filter(C::Kind.eq(kind.clone()));
        }
        if let Some(thread) = &self.thread {
            find = find.filter(C::ThreadKey.eq(thread.clone()));
        }
        let blocked = user.blocked_orgs();
        if !blocked.is_empty() {
            find = find.filter(C::OrgId.is_not_in(blocked.iter().copied()));
        }
        find
    }
}

#[cfg(test)]
mod tests {
    use axum::extract::Query;
    use axum::http::Uri;

    use super::*;

    fn parsed(query: &str) -> Option<FilterParams> {
        let uri: Uri = format!("/api/notifications/read-all?{query}")
            .parse()
            .unwrap();
        Query::<FilterParams>::try_from_uri(&uri).ok().map(|q| q.0)
    }

    #[test]
    fn an_unknown_parameter_is_refused_not_ignored() {
        let q = parsed("source=oxygen&category=runs&kind=oxy.run_failed&thread=ab").unwrap();
        assert_eq!(q.category.as_deref(), Some("runs"));
        // A typo would otherwise widen a scoped read-all to everything.
        assert!(parsed("source=oxygen&categroy=runs").is_none());
        assert!(parsed("org_id=not-a-uuid").is_none());
        assert!(parsed("").is_some());
    }
}
