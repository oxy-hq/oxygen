/** `value` as one POSIX shell word: single-quoted, an embedded `'` as `'\''`. */
export const shellQuote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;

/** VS Code's own "clone this repository" link; it asks where to put it. */
export const vscodeCloneUrl = (remote: string) =>
  `vscode://vscode.git/clone?url=${encodeURIComponent(remote)}`;

/**
 * The line people paste into a shell. The remote and branch come from the
 * workspace row, so both are quoted, and `--` ends the options before the URL.
 */
export const gitCloneCommand = (remote: string, branch: string | null | undefined) =>
  branch
    ? `git clone --branch ${shellQuote(branch)} -- ${shellQuote(remote)}`
    : `git clone -- ${shellQuote(remote)}`;

export const oxycCloneCommand = (workspaceId: string) =>
  `oxyc workspace clone ${shellQuote(workspaceId)}`;
