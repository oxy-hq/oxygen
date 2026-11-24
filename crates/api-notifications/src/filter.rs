//! The narrowing every inbox route shares.
//!
//! Always the person's own signals, in the orgs the request's token reaches.
//! Then, each optional and each narrowing: one org (`org_id`), one app
//! (`app_id`), one source (`source=oxygen|<app id>`), a category subtree of
//! that source (`category=<path>`, which needs `source`), one stream
//! (`stream=activity|operations`), one kind and one thread. The list, the badge
//! and `read-all` take the same parameters, so "mark all read" clears exactly
//! what the page shows.
//!
//! `stream` is the one that spans two columns: Activity is every app's notice
//! *plus* Oxygen's non-operator kinds, so it composes as a condition rather
//! than a chained filter. Its kind list comes from Oxygen's catalogue at
//! request time (`crate::stream::operations_kinds`), so adding a kind to the
//! platform table puts it in the right stream with no edit here.

use axum::http::StatusCode;
use entity::{apps, notifications};
use sea_orm::{ColumnTrait, Condition, DatabaseConnection, EntityTrait, QueryFilter};
use serde::Deserialize;
use uuid::Uuid;

use super::catalog::model::{GENERAL, valid_path};
use super::catalog::platform::oxygen;
use super::catalog::source::{KindSet, SourceCatalogue};
use super::input::valid_kind;
use super::stream::{Stream, operations_kinds};

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
    /// `activity` | `operations`. Anything else is `400`, like every other
    /// malformed parameter: `read-all` takes this filter, and a typo must not
    /// clear both streams.
    pub stream: Option<String>,
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
    /// The stream, with the Oxygen kinds Operations holds, read once.
    stream: Option<(Stream, Vec<String>)>,
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
        let stream = match self.stream.as_deref() {
            None => None,
            Some(value) => Some((
                Stream::parse(value).ok_or(StatusCode::BAD_REQUEST)?,
                operations_kinds(),
            )),
        };
        Ok(Filter {
            org_id: self.org_id,
            app_id: self.app_id,
            source,
            kinds,
            kind: self.kind.clone(),
            thread: self.thread.clone(),
            stream,
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
        if let Some((stream, operations)) = &self.stream {
            find = find.filter(stream_condition(*stream, operations));
        }
        let blocked = user.blocked_orgs();
        if !blocked.is_empty() {
            find = find.filter(C::OrgId.is_not_in(blocked.iter().copied()));
        }
        find
    }
}

/// `stream`'s narrowing: Activity is an app's notice or an Oxygen kind that is
/// not one of `operations`; Operations is the complement, within Oxygen.
///
/// Written over the two columns rather than as a kind list so that every app
/// kind — including ones no catalogue describes — is Activity by construction,
/// and so a kind this binary has never heard of lands in Activity, where
/// somebody is looking, instead of being hidden.
fn stream_condition(stream: Stream, operations: &[String]) -> Condition {
    use notifications::Column as C;
    match stream {
        Stream::Activity => Condition::any()
            .add(C::AppId.is_not_null())
            .add(C::Kind.is_not_in(operations.to_vec())),
        Stream::Operations => Condition::all()
            .add(C::AppId.is_null())
            .add(C::Kind.is_in(operations.to_vec())),
    }
}

#[cfg(test)]
mod tests {
    use axum::extract::Query;
    use axum::http::Uri;
    use sea_orm::QueryTrait;

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

    #[test]
    fn a_stream_parses_and_an_unknown_one_is_refused() {
        assert_eq!(
            parsed("stream=operations").unwrap().stream.as_deref(),
            Some("operations")
        );
        // Parsing is lenient (any string); resolving is what refuses, because
        // only there is the catalogue in hand. Both ends must say no.
        for bad in ["stream=ops", "stream=", "stream=Operations"] {
            let params = parsed(bad).unwrap();
            assert!(
                Stream::parse(params.stream.as_deref().unwrap_or("")).is_none(),
                "{bad}"
            );
        }
        assert!(
            parsed("streams=activity").is_none(),
            "a typo is 400, not ignored"
        );
    }

    #[test]
    fn activity_spans_both_columns_and_operations_is_oxygens_alone() {
        let operations = operations_kinds();
        assert!(!operations.is_empty(), "the condition would match nothing");
        let sql = |stream| {
            notifications::Entity::find()
                .filter(stream_condition(stream, &operations))
                .build(sea_orm::DatabaseBackend::Postgres)
                .to_string()
        };
        let activity = sql(Stream::Activity);
        assert!(
            activity.contains("\"app_id\" IS NOT NULL") && activity.contains("NOT IN"),
            "an app's notice OR not an operations kind: {activity}"
        );
        let ops = sql(Stream::Operations);
        assert!(
            ops.contains("\"app_id\" IS NULL") && ops.contains("\"kind\" IN"),
            "no app AND an operations kind: {ops}"
        );
    }
}
