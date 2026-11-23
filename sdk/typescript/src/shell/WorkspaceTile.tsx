import { useState } from "react";
import { cx } from "./cx";

/** Rail-top workspace identity: the org/code-first logo when present, the
 *  name initial otherwise. Pure branding — switching workspaces is the host
 *  app's concern (the rail `bottom` slot). No `title`: the name is beside it
 *  wherever it is drawn open (the rail's head, the nav sheet's), and closed it
 *  is the rail's toggle, whose own hint would sit under a native tooltip. */
export function WorkspaceTile({ name, logoUrl }: { name: string; logoUrl?: string }) {
  // Track the URL that failed (not a boolean) so a changed logoUrl —
  // workspace switch, cache-busted re-upload — retries instead of
  // latching the initial fallback for the component's lifetime.
  const [failedUrl, setFailedUrl] = useState<string | null>(null);
  const showLogo = !!logoUrl && failedUrl !== logoUrl;

  return (
    <span
      data-testid='rail-workspace'
      // The tile's fill backs the letter fallback; a real logo (often a
      // transparent PNG) sits on its own, so the fill would show through as a
      // grey square. The `--logo` modifier drops it — see shell.css.
      className={cx("oxy-shell-scope oxy-workspace-tile", showLogo && "oxy-workspace-tile--logo")}
    >
      {showLogo ? (
        <img
          src={logoUrl}
          alt={name}
          onError={() => setFailedUrl(logoUrl ?? null)}
          className='oxy-workspace-tile__img'
        />
      ) : (
        <span className='oxy-workspace-tile__letter'>{name.slice(0, 1).toUpperCase()}</span>
      )}
    </span>
  );
}
