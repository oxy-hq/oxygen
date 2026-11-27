//! The notes that are shown pictures, when the pictures are not taken, when
//! the model does not answer, and when there are more pictures than one
//! request carries. With the stub of `notes` in the model's place.

use agentic_core::delegation::TaskOutcome;
use agentic_pipeline::one_shot::ImageLimits;
use oxy_api_custom_apps::custom_apps_build_checks::ai::executor::run_guarded;

use super::notes::{
    Stub, check, failed_loads, png, read, roomy, settle, stores_files, task_of, verdicts,
};
use super::*;

/// A settled attempt with the one changed screen of `changes_detail`, its
/// files kept, and `loads` failed.
async fn settled(agent: &Agent, build_id: &str) -> Uuid {
    let built = draft(agent, build_id, None).await;
    settle(
        agent,
        built,
        stores_files(),
        failed_loads(),
        changes_detail(json!([])),
    )
    .await;
    built
}

/// Run the notes task of `built` with `stub`; its answer.
async fn write(agent: &Agent, built: Uuid, stub: &Stub) -> String {
    match run_guarded(&agent.t.db, &task_of(agent, built), Ok(stub)).await {
        TaskOutcome::Done { answer, .. } => answer,
        other => panic!("{other:?}"),
    }
}

/// The text of a check's one note.
fn note_text(answered: &Value, kind: &str) -> String {
    let notes = check(answered, kind)["notes"]
        .as_array()
        .expect("notes")
        .clone();
    assert_eq!(notes.len(), 1, "{kind}: {notes:?}");
    assert_eq!(notes[0]["author"], "model");
    notes[0]["text"].as_str().expect("text").to_string()
}

const NOT_SEEN: &str = "The model did not see the pictures";
const FROM_TEXT: &str = "this is written from each screen's text and numbers.";

/// A model whose provider has no image input is never sent a picture: both
/// notes are written from the text, and each opens by saying so.
#[tokio::test]
async fn a_model_that_takes_no_pictures_is_asked_in_words_and_the_notes_say_so() {
    let agent = gated().await;
    let built = settled(&agent, "pn-blind").await;
    let stub = Stub::new(None);
    write(&agent, built, &stub).await;

    assert!(stub.calls().iter().all(|call| call.pictures.is_empty()));
    let changes = &stub.asked("changes")[0];
    assert!(
        changes
            .text
            .contains("No picture is attached to this request")
    );
    assert!(
        changes.text.contains("Acme North: 9 open orders"),
        "the texts still go"
    );
    let answered = read(&agent, built).await;
    let why = "this workspace's model takes no pictures";
    assert_eq!(
        note_text(&answered, "changes"),
        format!("{NOT_SEEN} ({why}): {FROM_TEXT}\n\nchanges saw 0 pictures")
    );
    assert_eq!(
        note_text(&answered, "design_audit"),
        format!("{NOT_SEEN} ({why}): {FROM_TEXT}\n\naudit saw 0 pictures")
    );
    let audit = check(&answered, "design_audit");
    assert_eq!(audit["status"], "passed");
    assert_eq!(
        audit["summary"],
        "audited the CSS digest and the pictures of 0 of 1 changed screens"
    );
    // The note on a check and the summary never had pictures to miss.
    assert_eq!(note_text(&answered, "loads"), "check saw 0 pictures");
    assert_eq!(answered["summary"]["text"], "summary saw 0 pictures");
}

/// A provider that refuses the request that carried the pictures is asked
/// the same again without them, and is sent none after that.
#[tokio::test]
async fn a_provider_that_refuses_the_pictures_is_asked_again_without_them() {
    let agent = gated().await;
    let built = settled(&agent, "pn-refused").await;
    let stub = Stub {
        refuses_pictures: true,
        ..Stub::new(Some(roomy()))
    };
    write(&agent, built, &stub).await;

    let pictures = |what: &str| -> Vec<usize> {
        stub.asked(what)
            .iter()
            .map(|call| call.pictures.len())
            .collect()
    };
    assert_eq!(pictures("changes"), [2, 0], "refused, then asked in words");
    assert_eq!(pictures("audit"), [1, 0]);
    let answered = read(&agent, built).await;
    let why = "the model's provider refused them";
    assert_eq!(
        note_text(&answered, "changes"),
        format!("{NOT_SEEN} ({why}): {FROM_TEXT}\n\nchanges saw 0 pictures")
    );
    assert!(note_text(&answered, "design_audit").starts_with(&format!("{NOT_SEEN} ({why})")));
    assert_eq!(check(&answered, "design_audit")["status"], "passed");
}

/// A model call that fails changes no check: each note, the audit's row and
/// the summary say the note could not be written, by the failure's class.
#[tokio::test]
async fn a_failed_call_changes_no_status_and_every_note_says_so() {
    let agent = gated().await;
    let built = settled(&agent, "pn-failed").await;
    let before = read(&agent, built).await;
    let stub = Stub {
        fails: Some("rate_limit"),
        ..Stub::new(Some(roomy()))
    };
    let answer = write(&agent, built, &stub).await;
    assert_eq!(
        answer,
        "1 notes on checks that did not pass; changed screens: the model did not answer; \
         design audit: the model did not answer; summary: the model did not answer"
    );

    let after = read(&agent, built).await;
    assert_eq!(verdicts(&after), verdicts(&before), "no check was touched");
    assert_eq!(
        (&after["state"], &after["stalled"]),
        (&before["state"], &json!(null))
    );
    let failed = "the call to the model failed (rate_limit)";
    assert_eq!(
        note_text(&after, "loads"),
        format!(
            "No note was written on this check: {failed}. The check's result is as its runner \
             left it."
        )
    );
    assert_eq!(
        note_text(&after, "changes"),
        format!(
            "No note was written on the changed screens: {failed}. The comparison itself is as \
             its runner left it."
        )
    );
    let audit = check(&after, "design_audit");
    assert_eq!(
        (&audit["status"], &audit["advisory"], &audit["notes"]),
        (&json!("skipped"), &json!(true), &json!([]))
    );
    assert_eq!(audit["summary"], format!("not audited: {failed}"));
    assert_eq!(
        after["summary"]["text"],
        format!("No summary was written: {failed}.")
    );
    assert_eq!(after["summary"]["model"], "stub-vision-1");
}

/// Three changed screens for a provider that takes two pictures a request.
fn three_changed() -> (Value, Vec<(&'static str, Vec<u8>)>) {
    let screen = |path: &str, slug: &str, ratio: f64| {
        json!({ "path": path, "viewport": "desk", "changed": true, "ratio": ratio,
            "staging": format!("shot-staging-{slug}-desk.png"),
            "production": format!("shot-production-{slug}-desk.png"),
            "diff": format!("diff-{slug}-desk.png"),
            "production_ready": true, "production_errors": [] })
    };
    let detail = json!({ "production_build_id": "agent-prod", "compared": 3, "changed": 3,
        "screens": [screen("", "root", 0.5), screen("stores", "stores", 0.031),
            screen("reorder/plans", "reorder--plans", 0.2)],
        "seen": null });
    let files = vec![
        ("shot-staging-root-desk.png", png(1440, 4000, 101)),
        ("shot-production-root-desk.png", png(1440, 4000, 102)),
        ("shot-staging-stores-desk.png", png(1440, 900, 103)),
        ("shot-production-stores-desk.png", png(1440, 900, 104)),
        ("shot-staging-reorder--plans-desk.png", png(1440, 900, 105)),
        (
            "shot-production-reorder--plans-desk.png",
            png(1440, 900, 106),
        ),
    ];
    (detail, files)
}

/// More pictures than one request carries go in as many requests as the
/// provider's bounds take: every picture once, a screen's two together, and
/// the answers joined in order. No digest was kept, so the audit has only
/// the pictures.
#[tokio::test]
async fn more_pictures_than_one_request_carries_go_in_several() {
    let agent = gated().await;
    let built = draft(&agent, "pn-many", None).await;
    let (detail, files) = three_changed();
    settle(&agent, built, files, passed("loads", json!({})), detail).await;
    let two_a_request = ImageLimits {
        max_images: 2,
        ..roomy()
    };
    let stub = Stub::new(Some(two_a_request));
    let answer = write(&agent, built, &stub).await;
    assert!(
        answer.starts_with("0 notes on checks that did not pass"),
        "{answer}"
    );

    let sizes = |what: &str| -> Vec<Vec<usize>> {
        let calls = stub.asked(what);
        calls
            .iter()
            .map(|call| call.pictures.iter().map(|(_, bytes)| *bytes).collect())
            .collect()
    };
    assert_eq!(
        sizes("changes"),
        [vec![101, 102], vec![103, 104], vec![105, 106]],
        "a screen's two pictures ride together, each picture once"
    );
    assert_eq!(
        sizes("audit"),
        [vec![101, 103], vec![105]],
        "staging's only"
    );
    let asked = stub.asked("changes");
    assert!(
        asked[0]
            .text
            .starts_with("3 screens of this build differ from production.")
    );
    assert!(
        asked[1]
            .text
            .starts_with("More changed screens of the same build.")
    );
    assert!(
        asked[2]
            .text
            .contains("About screen \"reorder/plans\" at desk: 20.0% of its pixels differ.")
    );
    assert_eq!(
        asked[0].pictures[0].0,
        "Picture of the root screen at desk, on staging (the new build):"
    );

    let answered = read(&agent, built).await;
    assert_eq!(
        note_text(&answered, "changes"),
        "changes saw 2 pictures\n\nchanges saw 2 pictures\n\nchanges saw 2 pictures"
    );
    assert_eq!(
        note_text(&answered, "design_audit"),
        "audit saw 2 pictures\n\naudit saw 1 pictures"
    );
    assert_eq!(
        check(&answered, "design_audit")["summary"],
        "audited the pictures of 3 of 3 changed screens (no CSS digest was kept)"
    );
    assert_eq!(answered["state"], "passed");
}

/// A provider that refuses one request that carried pictures, without saying
/// its model takes none (a prompt too long with them, a picture it will not
/// read), is asked that request again in words. Nothing says it will refuse
/// the next: the later requests still carry their pictures, and the note
/// says which screens the model did not see.
#[tokio::test]
async fn a_refusal_of_one_request_does_not_cost_the_later_ones_their_pictures() {
    let agent = gated().await;
    let built = draft(&agent, "pn-once", None).await;
    let (detail, files) = three_changed();
    settle(&agent, built, files, passed("loads", json!({})), detail).await;
    let stub = Stub {
        refuses_one_request: true,
        ..Stub::new(Some(ImageLimits {
            max_images: 2,
            ..roomy()
        }))
    };
    write(&agent, built, &stub).await;

    let sizes = |what: &str| -> Vec<Vec<usize>> {
        let calls = stub.asked(what);
        calls
            .iter()
            .map(|call| call.pictures.iter().map(|(_, bytes)| *bytes).collect())
            .collect()
    };
    assert_eq!(
        sizes("changes"),
        [vec![101, 102], vec![], vec![103, 104], vec![105, 106]],
        "refused once, asked again in words, then shown the rest"
    );
    assert_eq!(sizes("audit"), [vec![101, 103], vec![105]]);
    let answered = read(&agent, built).await;
    assert_eq!(
        note_text(&answered, "changes"),
        format!(
            "The model saw the pictures of 2 of 3 screens only (the model's provider refused \
             them): {FROM_TEXT}\n\nchanges saw 0 pictures\n\nchanges saw 2 pictures\n\nchanges \
             saw 2 pictures"
        )
    );
    assert_eq!(
        note_text(&answered, "design_audit"),
        "audit saw 2 pictures\n\naudit saw 1 pictures"
    );
}
