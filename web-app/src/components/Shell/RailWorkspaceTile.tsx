import { useState } from "react";
import { workspaceLogoUrl } from "@/components/Shell/logoUrl";
import { cn } from "@/libs/shadcn/utils";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

/** Rail-top workspace identity: the org logo when present, the
 *  name initial otherwise. Pure branding — switching workspaces lives in
 *  the bottom cluster (`RailWorkspaceSwitch`). */
export function RailWorkspaceTile() {
  const { workspace } = useCurrentWorkspace();
  const orgUpdatedAt = useCurrentOrg((s) => s.org?.updated_at);
  const [logoFailed, setLogoFailed] = useState(false);

  const name = workspace?.name ?? "";
  const showLogo = !!workspace?.id && !logoFailed;

  return (
    <span
      data-testid='rail-workspace'
      title={name}
      // The tint backs the letter fallback; a real logo (often a transparent
      // PNG) sits on its own, so the fill would show through as a grey square.
      className={cn(
        "flex h-8 w-8 items-center justify-center overflow-hidden rounded-md",
        !showLogo && "bg-primary/10"
      )}
    >
      {showLogo && workspace ? (
        <img
          src={workspaceLogoUrl(workspace.id, orgUpdatedAt)}
          alt={name}
          onError={() => setLogoFailed(true)}
          className='h-full w-full object-contain'
        />
      ) : (
        <span className='font-bold text-primary text-xs'>{name.slice(0, 1).toUpperCase()}</span>
      )}
    </span>
  );
}
