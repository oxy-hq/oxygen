//! Sea-ORM query filter helpers for the `users` table.
//!
//! Used by `oxy-auth` and the platform's user-facing endpoints. Lives in
//! `oxy-platform` (rather than the original `oxy::database::filters` location)
//! so that leaf crates which need to read from `users` can avoid depending on
//! the full `oxy` crate.

use entity::users::{self, UserStatus};
use sea_orm::sea_query::{Expr, ExprTrait, Func, SimpleExpr};
use sea_orm::{ColumnTrait, Condition, QueryFilter, QueryOrder, Select};

pub struct UserFilters;

/// `lower(users.email) = lower(<email>)`.
///
/// An address is one mailbox whatever its capitals, and the ways in do not
/// agree on them: a magic link lowercases what was typed, while Google, GitHub
/// and Okta hand over whatever the provider holds. Matched exactly, the same
/// person signing in two ways became two accounts.
///
/// Both sides are lowered **by Postgres**. Lowering the argument in Rust would
/// put two definitions of "same letters" on either side of the `=`: Rust folds
/// by full Unicode rules, `LOWER()` by the database's collation, and they part
/// ways on non-ASCII capitals — where a row could stop matching its own exact
/// spelling. One function also means one index: `idx_users_email_lower`.
///
/// The same rule binds whoever **stores** an address: fold ASCII letters and
/// nothing else (`to_ascii_lowercase`). A full Unicode fold on the way in
/// writes a spelling `lower()` does not arrive at from the original, and the
/// account cannot be found by the address that made it.
fn email_matches(email: &str) -> SimpleExpr {
    ExprTrait::eq(
        Expr::expr(Func::lower(Expr::col((
            users::Entity,
            users::Column::Email,
        )))),
        Expr::expr(Func::lower(Expr::val(email))),
    )
}

impl UserFilters {
    pub fn active() -> Condition {
        Condition::all().add(users::Column::Status.eq(UserStatus::Active))
    }
    /// The user with this address, compared without regard to case.
    pub fn by_email(email: &str) -> Condition {
        Condition::all().add(email_matches(email))
    }
    pub fn active_by_email(email: &str) -> Condition {
        Condition::all()
            .add(users::Column::Status.eq(UserStatus::Active))
            .add(email_matches(email))
    }

    pub fn active_by_magic_link_token(token: &str) -> Condition {
        Condition::all()
            .add(users::Column::Status.eq(UserStatus::Active))
            .add(users::Column::MagicLinkToken.eq(token))
    }
}

/// Which row answers when more than one matches. Rows that differ only by case
/// already exist — they are the duplicates exact matching made — and each of
/// them is someone's account. So the row spelled exactly as asked comes first,
/// which is the row an exact match would have found: nobody who signs in the
/// way they always have is moved to the other account. With no exact spelling,
/// the oldest.
fn exact_spelling_first(
    select: Select<entity::users::Entity>,
    email: &str,
) -> Select<entity::users::Entity> {
    select
        .order_by_desc(ExprTrait::eq(
            Expr::col((users::Entity, users::Column::Email)),
            email,
        ))
        .order_by_asc(users::Column::CreatedAt)
}

pub trait UserQueryFilterExt<E>
where
    E: sea_orm::EntityTrait,
{
    fn filter_active(self) -> Select<E>;

    /// Users with this address, whatever its case; the exact spelling first.
    fn filter_by_email(self, email: &str) -> Select<E>;

    fn filter_active_by_email(self, email: &str) -> Select<E>;

    fn filter_active_by_magic_link_token(self, token: &str) -> Select<E>;
}

impl UserQueryFilterExt<entity::users::Entity> for Select<entity::users::Entity> {
    fn filter_active(self) -> Select<entity::users::Entity> {
        self.filter(UserFilters::active())
    }

    fn filter_by_email(self, email: &str) -> Select<entity::users::Entity> {
        exact_spelling_first(self.filter(UserFilters::by_email(email)), email)
    }

    fn filter_active_by_email(self, email: &str) -> Select<entity::users::Entity> {
        exact_spelling_first(self.filter(UserFilters::active_by_email(email)), email)
    }

    fn filter_active_by_magic_link_token(self, token: &str) -> Select<entity::users::Entity> {
        self.filter(UserFilters::active_by_magic_link_token(token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entity::prelude::Users;
    use sea_orm::{DatabaseBackend, EntityTrait, QueryTrait};

    fn sql(select: Select<entity::users::Entity>) -> String {
        select.build(DatabaseBackend::Postgres).to_string()
    }

    #[test]
    fn an_address_is_matched_without_regard_to_case() {
        let query = sql(Users::find().filter_by_email("Jane.Doe@Acme.com"));
        // Both sides lowered by the database, so one rule decides "same
        // letters" and the expression index on `lower(email)` serves it.
        assert!(
            query.contains(r#"LOWER("users"."email") = LOWER('Jane.Doe@Acme.com')"#),
            "{query}"
        );
    }

    #[test]
    fn the_exact_spelling_answers_before_any_other_then_the_oldest() {
        let query = sql(Users::find().filter_by_email("Jane.Doe@Acme.com"));
        let order = query.split("ORDER BY").nth(1).expect("an ORDER BY");
        // The argument as given, not lowered: it is what an exact match found.
        assert!(
            order.contains(r#""users"."email" = 'Jane.Doe@Acme.com' DESC"#),
            "{order}"
        );
        assert!(order.contains(r#""users"."created_at" ASC"#), "{order}");
        let exact = order.find("DESC").expect("exact first");
        let oldest = order.find("ASC").expect("then oldest");
        assert!(exact < oldest, "{order}");
    }

    #[test]
    fn the_active_variant_adds_the_status_and_keeps_both_rules() {
        let query = sql(Users::find().filter_active_by_email("Jane@Acme.com"));
        assert!(query.contains(r#""users"."status" ="#), "{query}");
        assert!(
            query.contains(r#"LOWER("users"."email") = LOWER('Jane@Acme.com')"#),
            "{query}"
        );
        assert!(query.contains("ORDER BY"), "{query}");
    }

    #[test]
    fn a_magic_link_token_is_still_matched_exactly() {
        let query = sql(Users::find().filter_active_by_magic_link_token("deadbeefcafe"));
        assert!(
            query.contains(r#""users"."magic_link_token" = 'deadbeefcafe'"#),
            "{query}"
        );
        assert!(!query.contains("LOWER"), "{query}");
    }
}
