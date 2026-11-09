use entity::experiment_results;
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use serde_json::json;

use super::fixtures::*;

#[tokio::test]
async fn second_result_row_for_a_run_is_rejected() {
    let db = setup_db().await;
    let (ws, _rev) = seed_workspace(&db, "table").await;
    let run = seed_run(&db, RunSeed::new(ws, "pilot", "closing")).await;

    assert!(
        insert_result_row(&db, run, false, json!({}), None).await,
        "the first row inserts"
    );
    assert!(
        !insert_result_row(&db, run, false, json!({}), None).await,
        "the unique run_id must turn a second insert into a conflict, not a second row"
    );
    let rows = experiment_results::Entity::find()
        .filter(experiment_results::Column::RunId.eq(run))
        .count(&db)
        .await
        .unwrap();
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn the_prediction_column_is_nullable_json_and_round_trips() {
    let db = setup_db().await;
    let (ws, _rev) = seed_workspace(&db, "prediction").await;
    let with = seed_run(&db, RunSeed::new(ws, "with_prior", "closing")).await;
    let without = seed_run(&db, RunSeed::new(ws, "without_prior", "closing")).await;

    let prediction = json!({ "predicted": 21.0, "measured": 17.4, "prior": { "value": 4.2, "source": "fitted" }, "prior_in_interval": null });
    assert!(
        insert_result_row_with(&db, with, false, json!({}), None, Some(prediction.clone())).await
    );
    assert!(insert_result_row(&db, without, false, json!({}), None).await);

    let read = |run| {
        experiment_results::Entity::find()
            .filter(experiment_results::Column::RunId.eq(run))
            .one(&db)
    };
    assert_eq!(
        read(with).await.unwrap().unwrap().prediction,
        Some(prediction),
        "a null `prior_in_interval` survives"
    );
    assert_eq!(
        read(without).await.unwrap().unwrap().prediction,
        None,
        "no prior frozen: the column is NULL"
    );
}
