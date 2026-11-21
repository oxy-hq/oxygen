//! `owner/repo` out of a workspace's `git_remote_url`.
//!
//! GitHub only. Every remote-backed workspace in production is on github.com
//! (read 2026-10-05), and the archive endpoint this feeds is GitHub's REST
//! API; a remote on another host is refused rather than guessed at.

/// The `(owner, repo)` a GitHub remote URL names, or `None` when the URL is
/// not a github.com repository. Accepts the forms git itself does: `https://`,
/// `ssh://`, and the scp-like `git@github.com:owner/repo`.
pub(super) fn github_slug(remote: &str) -> Option<(String, String)> {
    let remote = remote.trim();
    let after_scheme = match remote.split_once("://") {
        Some((_, rest)) => rest,
        // scp-like: `git@github.com:owner/repo.git`
        None => return slug_of(remote.split_once(':')?),
    };
    slug_of(after_scheme.split_once('/')?)
}

fn slug_of((authority, path): (&str, &str)) -> Option<(String, String)> {
    // Drop `user@` / `user:token@`, and a port.
    let host = authority.rsplit('@').next()?.split(':').next()?;
    if !host.eq_ignore_ascii_case("github.com") {
        return None;
    }
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    (is_name(owner) && is_name(repo)).then(|| (owner.to_string(), repo.to_string()))
}

/// What GitHub allows in an owner or repository name. Also what keeps either
/// from changing the shape of the URL it is interpolated into.
fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use super::github_slug;

    #[test]
    fn every_form_git_accepts_names_the_same_repository() {
        for remote in [
            "https://github.com/acme/analytics.git",
            "https://github.com/acme/analytics",
            "https://github.com/acme/analytics/",
            "https://x-access-token:ghs_abc@github.com/acme/analytics.git",
            "ssh://git@github.com/acme/analytics.git",
            "ssh://git@github.com:22/acme/analytics.git",
            "git@github.com:acme/analytics.git",
            "https://GitHub.com/acme/analytics.git",
        ] {
            let expected = Some(("acme".to_string(), "analytics".to_string()));
            assert_eq!(github_slug(remote), expected, "{remote}");
        }
    }

    #[test]
    fn a_remote_that_is_not_a_github_repository_is_refused() {
        for remote in [
            "https://gitlab.com/acme/analytics.git",
            "https://github.com.evil.example/acme/analytics.git",
            "https://github.com/acme",
            "https://github.com/acme/analytics/tree/main",
            "https://github.com/acme/../analytics",
            "https://github.com/ac me/analytics",
            "/srv/git/analytics.git",
            "",
        ] {
            assert_eq!(github_slug(remote), None, "{remote}");
        }
    }
}
