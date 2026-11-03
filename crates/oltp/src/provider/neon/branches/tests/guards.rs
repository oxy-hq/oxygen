//! Deleting a branch over real HTTP, against the same stub: what the client
//! refuses before a request, and what it refuses on Neon's own description of
//! the branch.

use super::*;

#[tokio::test]
async fn reset_and_delete_refuse_the_production_branch_before_any_call() {
    let stub = Arc::new(Stub::default());
    let neon = serve(stub.clone()).await;
    let err = neon.reset_branch(&staging(), "br-main").await.unwrap_err();
    assert!(
        matches!(err, ProviderError::BranchIsProduction(ref id) if id == "br-main"),
        "{err}"
    );
    let err = neon.delete_branch(&staging(), "br-main").await.unwrap_err();
    assert!(matches!(err, ProviderError::BranchIsProduction(_)), "{err}");
    assert!(
        stub.seen().is_empty(),
        "nothing may reach Neon: {:?}",
        stub.seen()
    );
}

#[tokio::test]
async fn delete_looks_first_and_waits_for_the_operation() {
    let stub = Arc::new(Stub::default());
    serve(stub.clone())
        .await
        .delete_branch(&staging(), "br-stg")
        .await
        .expect("delete");
    assert!(
        stub.has("GET", "/projects/cold-sky-123/branches/br-stg")
            .is_some()
    );
    assert!(
        stub.has("DELETE", "/projects/cold-sky-123/branches/br-stg")
            .is_some()
    );
    assert!(
        stub.has("GET", "/projects/cold-sky-123/operations/op-delete")
            .is_some()
    );

    let gone = Arc::new(Stub {
        delete_missing: true,
        ..Default::default()
    });
    serve(gone.clone())
        .await
        .delete_branch(&staging(), "br-stg")
        .await
        .expect("a 404 is already deleted");
    assert!(
        gone.has("DELETE", "/projects/cold-sky-123/branches/br-stg")
            .is_none()
    );
}

/// Neon's own description of the branch is the last word: a recorded id that
/// turns out to be the project's default branch, or a protected one, is a
/// corrupted row — refused before any DELETE, not left to Neon to refuse.
#[tokio::test]
async fn delete_refuses_what_neon_says_is_default_or_protected() {
    for (stub, id) in [
        // The stub reports `br-main` as the default branch.
        (Stub::default(), "br-main"),
        (
            Stub {
                protected: true,
                ..Default::default()
            },
            "br-stg",
        ),
    ] {
        let stub = Arc::new(stub);
        let mut req = staging();
        // Get past the id-level guard, so it is Neon's answer that refuses.
        req.parent_branch_id = "br-elsewhere".into();
        let err = serve(stub.clone())
            .await
            .delete_branch(&req, id)
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::BranchIsProduction(_)), "{err}");
        assert!(
            stub.seen().iter().all(|s| s.method != "DELETE"),
            "refused before any DELETE: {:?}",
            stub.seen()
        );
    }
}

/// `GET …/branches/{b}` answered, but not with something that proves the
/// branch is safe to delete: fail closed, never DELETE.
async fn refused_on(described: Value) -> ProviderError {
    let stub = Arc::new(Stub {
        described: Some(described),
        ..Default::default()
    });
    let err = serve(stub.clone())
        .await
        .delete_branch(&staging(), "br-stg")
        .await
        .expect_err("a description that proves nothing must not become a DELETE");
    assert!(
        stub.seen().iter().all(|s| s.method != "DELETE"),
        "refused before any DELETE: {:?}",
        stub.seen()
    );
    err
}

#[tokio::test]
async fn delete_fails_closed_on_a_description_it_cannot_read() {
    for body in [
        // No `branch` envelope at all.
        json!({ "id": "br-stg", "parent_id": "br-main", "default": false, "protected": false }),
        // No flags: nothing says it is not the default branch.
        json!({ "branch": { "id": "br-stg", "parent_id": "br-main" } }),
        json!({ "branch": { "id": "br-stg", "parent_id": "br-main", "default": false } }),
        // A flag of the wrong type.
        json!({ "branch": { "id": "br-stg", "parent_id": "br-main",
                            "default": "false", "protected": false } }),
        // A description of some other branch.
        json!({ "branch": { "id": "br-main", "parent_id": "br-main",
                            "default": false, "protected": false } }),
    ] {
        let err = refused_on(body.clone()).await;
        assert!(matches!(err, ProviderError::Api { .. }), "{body}: {err}");
    }
}

#[tokio::test]
async fn delete_refuses_a_branch_not_cut_from_production() {
    for (parent, body) in [
        (
            "br-dev",
            json!({ "branch": { "id": "br-stg", "parent_id": "br-dev",
                                "default": false, "protected": false } }),
        ),
        // No parent: a root branch, which staging never is.
        (
            "",
            json!({ "branch": { "id": "br-stg", "default": false, "protected": false } }),
        ),
    ] {
        let err = refused_on(body).await;
        assert!(
            matches!(err, ProviderError::BranchParentMismatch { parent: ref p, .. } if p == parent),
            "{err}"
        );
    }
}

/// Neon reports a schema-only branch with no parent, so "cut from production"
/// cannot be the test for it — but only when the request says that is what it
/// expects. The same body for a data branch is a root branch that is not ours.
#[tokio::test]
async fn a_schema_only_branch_with_no_parent_is_deletable() {
    let described = json!({ "branch": { "id": "br-stg", "default": false, "protected": false } });
    let stub = Arc::new(Stub {
        described: Some(described),
        ..Default::default()
    });
    let mut req = staging();
    req.init = BranchInit::SchemaOnly;
    serve(stub.clone())
        .await
        .delete_branch(&req, "br-stg")
        .await
        .expect("a parentless schema-only branch is the expected shape");
    assert!(
        stub.has("DELETE", "/projects/cold-sky-123/branches/br-stg")
            .is_some(),
        "the delete is sent"
    );
}

#[tokio::test]
async fn a_data_branch_with_no_parent_is_refused() {
    let err =
        refused_on(json!({ "branch": { "id": "br-stg", "default": false, "protected": false } }))
            .await;
    assert!(
        matches!(err, ProviderError::BranchParentMismatch { .. }),
        "{err}"
    );
}

/// A schema-only request adopts only a parentless branch. The orphan under the
/// name was cut from production — a data branch an earlier attempt in the other
/// mode left — and adopting it would serve production's rows as "empty".
#[tokio::test]
async fn a_schema_only_create_never_adopts_a_data_branch() {
    let stub = Arc::new(Stub {
        orphan: true,
        ..Default::default()
    });
    let mut req = staging();
    req.init = BranchInit::SchemaOnly;
    let err = serve(stub.clone())
        .await
        .create_branch(&req)
        .await
        .unwrap_err();
    assert!(
        matches!(err, ProviderError::BranchParentMismatch { ref parent, .. } if parent == "br-main"),
        "{err}"
    );
    assert!(
        stub.has("POST", "/projects/cold-sky-123/branches")
            .is_none(),
        "nothing created under the name either"
    );
}
