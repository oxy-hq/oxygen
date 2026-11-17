import { Navigate, useLocation } from "react-router-dom";

/**
 * Where a retired admin route now lives — for the bookmarks, emails and pasted links
 * that still point at it.
 *
 * Carries the old URL's query and hash across: the Monday usage email links to anchors
 * in the report (`/admin/usage-report#org-…`), and a bare `<Navigate>` would land the
 * reader at the top of the right page instead of on the org the email was about. The
 * target's own query wins over a key the old URL also set.
 */
export const AdminRedirect = ({ to }: { to: string }) => {
  const { search, hash } = useLocation();
  return <Navigate replace to={mergeLocation(to, search, hash)} />;
};

/** `to`, with `search`'s keys added where `to` does not set them, and `hash` appended. */
export function mergeLocation(to: string, search: string, hash: string): string {
  const [path, query = ""] = to.split("?");
  const params = new URLSearchParams(query);
  new URLSearchParams(search).forEach((value, key) => {
    if (!params.has(key)) params.set(key, value);
  });
  const qs = params.toString();
  return `${path}${qs ? `?${qs}` : ""}${hash}`;
}
