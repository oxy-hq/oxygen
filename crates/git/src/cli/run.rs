use std::path::Path;

use oxy_shared::errors::OxyError;
use tokio::process::Command;

use crate::cli::{auth, redact};
use crate::types::Auth;

/// Make a git invocation self-contained: it authenticates with the credentials
/// Oxy hands it, or with none at all.
///
/// **`credential.helper=`** (empty value) resets the accumulated helper list,
/// including URL-scoped entries such as
/// `[credential "https://github.com"] helper = !gh auth git-credential`.
/// Without it, a token-less invocation silently borrows whatever the host has
/// configured — a `gh` helper reading `GH_TOKEN`, `osxkeychain`, a
/// `.git-credentials` file. That fallback is not a feature: it makes a fetch
/// succeed or fail based on the environment the server process happened to be
/// launched from rather than on the GitHub connection the org configured, so a
/// workspace with no linked connection works on the developer's laptop and
/// fails everywhere else. Whoever the host credentials belong to is also not
/// necessarily whoever is making the request.
///
/// **`GIT_TERMINAL_PROMPT=0`** then makes the no-credentials case fail fast
/// instead of hanging on `Username for 'https://...':`.
///
/// Note this governs HTTP(S) auth only. An SSH remote still uses the host's
/// keys — that is an explicitly provisioned deploy mechanism, not an implicit
/// fallback, and git offers no equivalent reset for it.
fn harden(cmd: &mut Command) {
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.arg("-c").arg("credential.helper=");
}

/// Turn a captured failure into an `OxyError`, with secrets stripped.
fn failure(args: &[&str], stderr: &[u8]) -> OxyError {
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = redact::redact_secrets(&stderr);
    OxyError::RuntimeError(format!("git {} failed: {stderr}", args.join(" ")))
}

/// Run `git <args>` in `cwd`, no auth. Returns captured stdout on success.
pub(crate) async fn run(cwd: &Path, args: &[&str]) -> Result<String, OxyError> {
    run_authed(cwd, args, &Auth::None).await
}

pub(crate) async fn run_with_token(
    cwd: &Path,
    args: &[&str],
    token: Option<&str>,
) -> Result<String, OxyError> {
    match token {
        Some(t) => run_authed(cwd, args, &Auth::bearer(t)).await,
        None => run(cwd, args).await,
    }
}

/// Like [`run`] but sets `GIT_EDITOR=true` so git never opens an editor —
/// used by `rebase --continue` / `merge --continue`.
pub(crate) async fn run_no_editor(cwd: &Path, args: &[&str]) -> Result<String, OxyError> {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd);
    cmd.env("GIT_EDITOR", "true");
    harden(&mut cmd);
    cmd.args(args);

    let output = cmd.output().await.map_err(|e| {
        OxyError::RuntimeError(format!("Failed to execute git {}: {e}", args.join(" ")))
    })?;

    if !output.status.success() {
        return Err(failure(args, &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run `git <args>` in `cwd` with auth injected via `http.extraHeader`.
/// See [`harden`] for why a token-less run does not silently fall back to the
/// host machine's git credentials.
pub(crate) async fn run_authed(
    cwd: &Path,
    args: &[&str],
    auth_: &Auth,
) -> Result<String, OxyError> {
    spawn_and_wait(cwd, args, auth_, false).await
}

/// [`run_authed`] for a command that is safe to abandon: dropping the future
/// kills the git child. Only for commands that write nothing shared until
/// they finish, such as `clone` into a fresh directory. A mutating command on
/// an existing repository (`pull --rebase`, `commit`, `reset`) must run to
/// completion even when its request is dropped, or it leaves `index.lock` or a
/// half-applied rebase behind; those use [`run_authed`].
pub(crate) async fn run_authed_cancellable(
    cwd: &Path,
    args: &[&str],
    auth_: &Auth,
) -> Result<String, OxyError> {
    spawn_and_wait(cwd, args, auth_, true).await
}

async fn spawn_and_wait(
    cwd: &Path,
    args: &[&str],
    auth_: &Auth,
    kill_on_drop: bool,
) -> Result<String, OxyError> {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd);
    harden(&mut cmd);
    auth::apply(&mut cmd, auth_);
    cmd.args(args);
    cmd.kill_on_drop(kill_on_drop);

    let output = cmd.output().await.map_err(|e| {
        OxyError::RuntimeError(format!("Failed to execute git {}: {e}", args.join(" ")))
    })?;

    if !output.status.success() {
        return Err(failure(args, &output.stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// Unix-only: the fixture writes a `#!/bin/sh` credential helper, so these
// would fail rather than skip on Windows.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    /// A `HOME` holding a gitconfig whose GitHub credentials come from a helper
    /// — the shape a developer machine actually has (`gh auth git-credential`,
    /// `osxkeychain`). URL-scoped on purpose: a plain `credential.helper` reset
    /// has to clear these too, and that is the case worth pinning.
    fn fake_helper_home() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let helper = home.path().join("helper.sh");
        let mut f = std::fs::File::create(&helper).unwrap();
        writeln!(f, "#!/bin/sh\n[ \"$1\" = get ] || exit 0").unwrap();
        writeln!(f, "echo username=host-user\necho password=host-secret").unwrap();
        drop(f);
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(
            home.path().join(".gitconfig"),
            format!(
                "[credential \"https://github.com\"]\n\thelper = {}\n",
                helper.display()
            ),
        )
        .unwrap();
        home
    }

    /// Ask git which credentials it would send to github.com, optionally
    /// hardened the way every Oxy invocation is.
    async fn credentials_git_would_use(home: &std::path::Path, hardened: bool) -> String {
        let mut cmd = Command::new("git");
        cmd.env("HOME", home)
            .env("XDG_CONFIG_HOME", home)
            // Otherwise the developer's real global config leaks into the test.
            .env_remove("GIT_CONFIG_GLOBAL")
            .env_remove("GIT_CONFIG_COUNT");
        if hardened {
            harden(&mut cmd);
        }
        cmd.args(["credential", "fill"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"protocol=https\nhost=github.com\n\n")
            .await
            .unwrap();
        let out = child.wait_with_output().await.unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// The control: without hardening, git happily hands over the host's
    /// credentials. This is what used to make a token-less fetch "work" on a
    /// developer laptop and fail on every other machine.
    ///
    /// Not redundant with the test below, despite looking it. That one only
    /// inspects stdout, so on its own it would also pass with no `git` on the
    /// box or a fixture that stopped exercising the helper. This one fails
    /// loudly in both cases, which is what keeps the guarantee honest.
    #[tokio::test]
    async fn host_credential_helper_supplies_credentials_by_default() {
        let home = fake_helper_home();
        let creds = credentials_git_would_use(home.path(), false).await;
        assert!(
            creds.contains("password=host-secret"),
            "test fixture is not exercising the helper at all: {creds:?}"
        );
    }

    /// The guarantee: a hardened invocation gets nothing from the host, so an
    /// operation Oxy could not supply a token for fails loudly instead of
    /// silently borrowing whoever's credentials the box happens to hold.
    #[tokio::test]
    async fn hardened_invocation_ignores_host_credential_helper() {
        let home = fake_helper_home();
        let creds = credentials_git_would_use(home.path(), true).await;
        assert!(
            !creds.contains("host-secret"),
            "host credentials leaked into a hardened git invocation: {creds:?}"
        );
    }
}
