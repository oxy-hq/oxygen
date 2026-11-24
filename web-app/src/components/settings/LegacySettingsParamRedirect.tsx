import { useEffect } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import useCurrentOrg from "@/stores/useCurrentOrg";
import { SETTINGS_PARAM, settingsHref, targetFromId } from "./settingsRoute";

/**
 * Sends `?settings=<id>` to the page it now names, and keeps doing so forever.
 *
 * Settings used to be a dialog this parameter opened. It is a route now, but the parameter is
 * **not** a legacy of the web app alone: the Rust backend writes it into notification emails
 * already sent (billing, token hygiene, the notifications digest), published `oxyc` versions
 * print it for `tokens` and `init-ci`, and the custom-app shell emits it. Those links cannot
 * be reissued, so this redirect is permanent, not a migration step.
 *
 * It accepts everything the dialog accepted, including the four ids that moved
 * (`RENAMED_SECTIONS`) and a tab as a third segment. An id naming nothing is dropped rather
 * than sent somewhere arbitrary, and other query parameters are left alone — `?slack_installed`
 * rides along with it on the Slack return.
 */
export function LegacySettingsParamRedirect() {
  const [searchParams, setSearchParams] = useSearchParams();
  const navigate = useNavigate();
  const params = useParams();
  const storeSlug = useCurrentOrg((s) => s.org?.slug);
  const orgSlug = params.orgSlug ?? storeSlug;

  const raw = searchParams.get(SETTINGS_PARAM);

  useEffect(() => {
    if (raw === null) return;
    const target = targetFromId(raw);
    if (!target) {
      // Not a page we have: drop the parameter rather than leave it in the address.
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.delete(SETTINGS_PARAM);
          return next;
        },
        { replace: true }
      );
      return;
    }
    const rest = new URLSearchParams(searchParams);
    rest.delete(SETTINGS_PARAM);
    const search = rest.toString();
    navigate(
      {
        pathname: settingsHref(orgSlug, target.section, target.tab ?? null),
        search: search ? `?${search}` : ""
      },
      // Replace, so Back goes where the person was before the link, not to a URL that
      // immediately redirects again.
      { replace: true }
    );
  }, [raw, searchParams, setSearchParams, navigate, orgSlug]);

  return null;
}
