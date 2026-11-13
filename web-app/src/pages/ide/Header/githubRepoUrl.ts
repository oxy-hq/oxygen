/** `https://github.com/<owner>/<repo>` for a GitHub remote (https or ssh), else null. */
export function githubRepoBase(remoteUrl?: string | null): string | null {
  if (!remoteUrl) return null;
  const match = remoteUrl.match(/github\.com[/:]([^/]+\/[^/.]+?)(?:\.git)?$/);
  return match ? `https://github.com/${match[1]}` : null;
}

interface Args {
  remoteUrl?: string | null;
  branch: string;
  /** The workspace's directory in its repository, when it is not the root. */
  subdir?: string | null;
}

/**
 * The workspace's tree on GitHub at `branch`, or null when the remote is not a
 * GitHub URL. A workspace in a subdirectory links to that directory.
 */
export function githubRepoUrl({ remoteUrl, branch, subdir }: Args): string | null {
  const base = githubRepoBase(remoteUrl);
  if (!base) return null;
  const encodedSubdir = subdir ? subdir.split("/").map(encodeURIComponent).join("/") : null;
  return `${base}/tree/${encodedSubdir ? `${branch}/${encodedSubdir}` : branch}`;
}

/** The commit on GitHub, or null when the remote is not a GitHub URL. */
export function githubCommitUrl(remoteUrl: string | null | undefined, sha: string): string | null {
  const base = githubRepoBase(remoteUrl);
  return base && sha ? `${base}/commit/${encodeURIComponent(sha)}` : null;
}
