/** `https://github.com/<owner>/<repo>` for a GitHub remote (https or ssh), else null. */
export function githubRepoBase(remoteUrl?: string | null): string | null {
  if (!remoteUrl) return null;
  const match = remoteUrl.match(/github\.com[/:]([^/]+\/[^/.]+?)(?:\.git)?$/);
  return match ? `https://github.com/${match[1]}` : null;
}

interface Args {
  remoteUrl?: string | null;
  branch: string;
  defaultBranch: string;
  isOnMain: boolean;
  gitSubfolder?: string;
}

/**
 * Derive `repo` and `pr` URLs from a GitHub remote URL.
 * Returns `{ repoUrl: null, prUrl: null }` when the remote isn't a GitHub URL
 * or when no remote is configured. The PR URL is null on the default branch
 * (no compare target).
 *
 * When the workspace lives in a subdirectory of the git repository,
 * `gitSubfolder` is appended to the repo URL so the link opens the correct
 * directory rather than the repository root.
 */
export function useGithubUrls({ remoteUrl, branch, defaultBranch, isOnMain, gitSubfolder }: Args) {
  const base = githubRepoBase(remoteUrl);

  if (!base) return { repoUrl: null, prUrl: null };
  const encodedSubfolder = gitSubfolder
    ? gitSubfolder.split("/").map(encodeURIComponent).join("/")
    : null;
  const treePath = encodedSubfolder ? `${branch}/${encodedSubfolder}` : branch;
  return {
    repoUrl: `${base}/tree/${treePath}`,
    prUrl: isOnMain ? null : `${base}/compare/${defaultBranch}...${branch}?expand=1`
  };
}
