//! Which rows are standing tokens, and that the staff list asks the database
//! for exactly those.

use chrono::Utc;
use sea_orm::{DbBackend, QueryTrait};

use super::*;
use crate::token::credential::source;

/// A token of `kind` minted by the tokens API, carrying the standings given.
fn row(kind: StoredKind, platform: bool, partner: bool) -> api_tokens::Model {
    api_tokens::Model {
        id: Uuid::new_v4(),
        kind: kind.as_str().to_string(),
        principal_user_id: Uuid::new_v4(),
        name: "laptop".into(),
        display_prefix: "oxy_pat_Ab3x".into(),
        last_four: "wxyz".into(),
        token_hash: vec![0; 32],
        all_access: true,
        platform,
        partner,
        expires_at: None,
        last_used_at: None,
        created_at: Utc::now().fixed_offset(),
        created_by: None,
        revoked_at: None,
        revoked_by: None,
        revoke_reason: None,
        source: source::UI.into(),
        legacy_api_key_id: None,
        trust_policy_id: None,
        oidc_claims: None,
    }
}

fn sql() -> String {
    select(500).build(DbBackend::Postgres).to_string()
}

#[test]
fn a_personal_token_with_either_standing_is_one() {
    assert!(carries_standing(&row(StoredKind::Personal, true, false)));
    assert!(carries_standing(&row(StoredKind::Personal, false, true)));
    assert!(carries_standing(&row(StoredKind::Personal, true, true)));
    // An ordinary personal token reaches what its owner's memberships do.
    assert!(!carries_standing(&row(StoredKind::Personal, false, false)));
}

#[test]
fn a_revoked_or_grant_bound_one_is_still_one() {
    let revoked = api_tokens::Model {
        revoked_at: Some(Utc::now().fixed_offset()),
        ..row(StoredKind::Personal, true, false)
    };
    assert!(carries_standing(&revoked), "an ended token stays listed");
    let bound = api_tokens::Model {
        all_access: false,
        ..row(StoredKind::Personal, true, false)
    };
    assert!(carries_standing(&bound));
}

#[test]
fn no_other_kind_is_one_whatever_its_row_stores() {
    for kind in [
        StoredKind::LegacyKey,
        StoredKind::ServiceAccount,
        StoredKind::Ci,
        StoredKind::SandboxAgent,
    ] {
        assert!(!carries_standing(&row(kind, true, true)), "{kind:?}");
    }
    assert!(!carries_standing(&api_tokens::Model {
        kind: "a kind a later release adds".into(),
        ..row(StoredKind::Personal, true, true)
    }));
}

/// A row that mirrors `api_keys` stores both standings always. It is a legacy
/// key even when its kind reads `personal`, and these routes never reach one.
#[test]
fn a_row_that_mirrors_an_api_key_is_never_one() {
    let mirrored = api_tokens::Model {
        source: source::LEGACY_ENDPOINT.into(),
        legacy_api_key_id: Some(Uuid::new_v4()),
        ..row(StoredKind::Personal, true, true)
    };
    assert!(!carries_standing(&mirrored));
}

#[test]
fn the_list_asks_for_what_the_predicate_says() {
    let sql = sql();
    for clause in [
        r#""api_tokens"."kind" = 'personal'"#,
        r#""api_tokens"."legacy_api_key_id" IS NULL"#,
        r#"("api_tokens"."platform" = TRUE OR "api_tokens"."partner" = TRUE)"#,
    ] {
        assert!(sql.contains(clause), "missing `{clause}` in: {sql}");
    }
    // Ended tokens are listed: the query both halves share filters on neither
    // a revocation nor an expiry.
    for column in ["revoked_at", "expires_at"] {
        let filtered = sql
            .split_once(" WHERE ")
            .is_some_and(|(_, filters)| filters.contains(column));
        assert!(!filtered, "the list filters on {column}: {sql}");
    }
}

#[test]
fn the_newest_rows_come_first_and_the_limit_is_in_the_query() {
    let sql = sql();
    assert!(
        sql.ends_with(
            r#"ORDER BY "api_tokens"."created_at" DESC, "api_tokens"."id" DESC LIMIT 500"#
        ),
        "{sql}"
    );
}

/// A standing token is a credential for the whole deployment, so the list is
/// never a subset by org: it reads one table and names no grant.
#[test]
fn the_list_is_not_narrowed_by_org() {
    let sql = sql();
    assert!(!sql.contains("api_token_grants"), "{sql}");
    assert!(!sql.contains("org_id"), "{sql}");
}

/// The list reads the tokens that still work first, so the ended ones — which
/// every `oxyc login` adds to — can never push one of them off the end.
#[test]
fn the_working_half_asks_for_tokens_neither_revoked_nor_expired() {
    let now = Utc::now().fixed_offset();
    let sql = select(500)
        .filter(works_at(now))
        .build(DbBackend::Postgres)
        .to_string();
    assert!(
        sql.contains(r#""api_tokens"."revoked_at" IS NULL"#),
        "{sql}"
    );
    assert!(
        sql.contains(r#"("api_tokens"."expires_at" IS NULL OR "api_tokens"."expires_at" > "#),
        "a token with no expiry works, and so does one whose expiry is ahead: {sql}"
    );
}

/// The ended half is the working half negated and nothing else, so a token is
/// in exactly one of them.
#[test]
fn the_ended_half_is_the_working_half_negated() {
    let now = Utc::now().fixed_offset();
    let working = select(500)
        .filter(works_at(now))
        .build(DbBackend::Postgres)
        .to_string();
    let ended = select(500)
        .filter(works_at(now).not())
        .build(DbBackend::Postgres)
        .to_string();
    // Within the filters: the column list names `revoked_at` too.
    let clause = |sql: &str| {
        let (_, filters) = sql.split_once(" WHERE ").expect("the filters");
        let from = filters
            .find(r#""api_tokens"."revoked_at""#)
            .expect("the clause");
        let to = filters.find(" ORDER BY ").expect("the order");
        filters[from..to].to_string()
    };
    assert!(
        ended.contains(&format!("NOT ({})", clause(&working))),
        "{ended}"
    );
}

#[test]
fn the_two_halves_come_back_as_one_list_newest_first() {
    let at = |hours_ago: i64, revoked: bool| api_tokens::Model {
        created_at: (Utc::now() - chrono::Duration::hours(hours_ago)).fixed_offset(),
        revoked_at: revoked.then(|| Utc::now().fixed_offset()),
        ..row(StoredKind::Personal, true, false)
    };
    let (old_working, new_working) = (at(90, false), at(2, false));
    let (newest_ended, older_ended) = (at(1, true), at(30, true));
    let listed = newest_first(
        vec![new_working.clone(), old_working.clone()],
        vec![newest_ended.clone(), older_ended.clone()],
    );
    let ids: Vec<Uuid> = listed.iter().map(|t| t.id).collect();
    assert_eq!(
        ids,
        vec![
            newest_ended.id,
            new_working.id,
            older_ended.id,
            old_working.id
        ]
    );
}

/// The two halves are two reads. A token revoked between them comes back in
/// both, and must be listed once, as ended.
#[test]
fn a_token_revoked_between_the_two_reads_is_listed_once_as_ended() {
    let before = row(StoredKind::Personal, true, false);
    let after = api_tokens::Model {
        revoked_at: Some(Utc::now().fixed_offset()),
        ..before.clone()
    };
    let other = row(StoredKind::Personal, false, true);
    let listed = newest_first(vec![before, other.clone()], vec![after.clone()]);
    assert_eq!(listed.len(), 2, "{listed:?}");
    let again = listed.iter().find(|t| t.id == after.id).expect("listed");
    assert!(again.revoked_at.is_some(), "the later fact is the one kept");
    assert!(listed.iter().any(|t| t.id == other.id));
}
