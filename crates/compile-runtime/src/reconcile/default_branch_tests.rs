use super::*;

/// A fake GitHub: answers one repository, or a fixed error for every one.
struct FakeGitHub {
    answer: Result<String, DefaultBranchError>,
    asked: std::sync::Mutex<Vec<String>>,
}

#[async_trait]
impl DefaultBranchSource for FakeGitHub {
    async fn default_branch(
        &self,
        owner: &str,
        repo: &str,
        _timeout: Duration,
    ) -> Result<String, DefaultBranchError> {
        self.asked.lock().unwrap().push(format!("{owner}/{repo}"));
        self.answer.clone()
    }
}

const ASK: Duration = Duration::from_secs(5);

fn fake(answer: Result<String, DefaultBranchError>) -> FakeGitHub {
    FakeGitHub {
        answer,
        asked: std::sync::Mutex::new(Vec::new()),
    }
}

fn workspace(remote: Option<&str>) -> workspaces::Model {
    let now = chrono::Utc::now().fixed_offset();
    workspaces::Model {
        id: uuid::Uuid::new_v4(),
        name: "ws".into(),
        git_namespace_id: None,
        git_remote_url: remote.map(str::to_string),
        created_at: now,
        updated_at: now,
        path: None,
        last_opened_at: None,
        created_by: None,
        org_id: None,
        status: workspaces::WorkspaceStatus::Ready,
        error: None,
        monthly_vlm_budget_micros: None,
        current_revision_id: None,
        default_branch: None,
        repo_subdir: None,
        default_branch_attempted_at: None,
    }
}

#[tokio::test]
async fn a_github_remote_learns_the_repositorys_default_branch() {
    let github = fake(Ok("trunk".into()));
    let row = workspace(Some("https://github.com/acme/analytics.git"));
    assert_eq!(
        learn(&github, &row, ASK).await,
        Learned::Branch("trunk".into())
    );
    assert_eq!(*github.asked.lock().unwrap(), ["acme/analytics"]);
}

#[tokio::test]
async fn a_remote_that_is_not_github_asks_nobody() {
    let github = fake(Ok("main".into()));
    for remote in [None, Some("https://gitlab.com/acme/analytics.git")] {
        assert_eq!(
            learn(&github, &workspace(remote), ASK).await,
            Learned::Nothing
        );
    }
    assert!(github.asked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_unanswered_question_records_nothing_and_an_unwell_github_stops_the_pass() {
    let row = workspace(Some("git@github.com:acme/analytics.git"));
    let missing = fake(Err(BranchHeadError::NotFound {
        owner: "acme".into(),
        repo: "analytics".into(),
        branch: "(default)".into(),
    }
    .into()));
    assert_eq!(learn(&missing, &row, ASK).await, Learned::Nothing);
    let limited = fake(Err(BranchHeadError::RateLimited { status: 429 }.into()));
    assert_eq!(learn(&limited, &row, ASK).await, Learned::RateLimited);
    let down = fake(Err(
        BranchHeadError::Unavailable("connection reset".into()).into()
    ));
    assert_eq!(learn(&down, &row, ASK).await, Learned::Stop);
}

/// An answer about this one repository — an unmapped status, a body naming no
/// branch — is held against the repository, not against GitHub: the pass goes
/// on to the next workspace.
#[tokio::test]
async fn an_answer_without_a_branch_does_not_stop_the_pass() {
    let row = workspace(Some("https://github.com/acme/analytics.git"));
    for why in [
        "HTTP 451 from …",
        "HTTP 502 from …",
        "the repository body names no default_branch",
    ] {
        let odd = fake(Err(DefaultBranchError::NoAnswer(why.into())));
        assert_eq!(learn(&odd, &row, ASK).await, Learned::Nothing, "{why}");
    }
}

/// Never answers.
struct Silent;

#[async_trait]
impl DefaultBranchSource for Silent {
    async fn default_branch(
        &self,
        _owner: &str,
        _repo: &str,
        _timeout: Duration,
    ) -> Result<String, DefaultBranchError> {
        std::future::pending().await
    }
}

/// A lookup gets what is left of the pass's budget and no more: a GitHub that
/// does not answer stops the pass when the budget is spent.
#[tokio::test(start_paused = true)]
async fn a_lookup_is_cut_off_at_the_remaining_budget() {
    let row = workspace(Some("https://github.com/acme/analytics.git"));
    let started = tokio::time::Instant::now();
    assert_eq!(
        learn(&Silent, &row, Duration::from_secs(2)).await,
        Learned::Stop
    );
    assert_eq!(started.elapsed(), Duration::from_secs(2));
}
