/**
 * `/<org>/workspaces/<id>/<page>` → `/<org>/<page>`.
 *
 * The address of an org's pages no longer names a workspace (`OrgWorkspace`),
 * but the old one stays live for good: emails, Slack messages, push
 * notifications and bookmarks carry it, and the server still builds it where a
 * link must open one particular workspace. This is where such a link lands.
 *
 * Works on the raw pathname rather than the router's decoded splat, so an
 * encoded segment (an experiment name with a space, a base64 path) arrives as
 * it was sent. Returns null for a path that is not an entry link.
 */
const ENTRY = /^\/([^/]+)\/workspaces\/[^/]+(?:\/(.*))?$/;

/** Workspace pages whose address changed with the move. `/<org>/onboarding` is
 *  the org's retired wizard, so the workspace's setup page became `setup`. */
const RENAMED: Readonly<Record<string, string>> = { onboarding: "setup" };

export function entryRedirectPath(pathname: string): string | null {
  const match = ENTRY.exec(pathname);
  if (!match) return null;
  const [, orgSlug, rest = ""] = match;
  const page = rest.replace(/\/+$/, "");
  // The workspace's root was its home; the org's root is the org dispatcher.
  if (page === "") return `/${orgSlug}/home`;
  const [first, ...more] = page.split("/");
  return `/${orgSlug}/${[RENAMED[first] ?? first, ...more].join("/")}`;
}
