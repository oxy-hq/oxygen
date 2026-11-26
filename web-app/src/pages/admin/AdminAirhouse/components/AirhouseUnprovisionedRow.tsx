import { Warehouse } from "lucide-react";
import { useState } from "react";
import type { AirhouseFleetRow as Row } from "@/services/api/airhouseAdmin";
import { ProvisionConfirmDialog } from "./ProvisionConfirmDialog";

/** A workspace with no warehouse: a name and the one action that applies. */
export const AirhouseUnprovisionedRow = ({ row }: { row: Row }) => {
  const [confirming, setConfirming] = useState(false);
  return (
    <div
      className='flex items-center justify-between gap-2 border-border/60 border-b px-1 py-1 last:border-b-0'
      data-testid={`admin-airhouse-row-${row.workspace_id}`}
    >
      <span className='min-w-0 truncate text-xs'>
        {row.workspace_name}
        {row.org_name ? <span className='ml-1.5 text-muted-foreground'>{row.org_name}</span> : null}
      </span>
      {/* Plain button, not `<Button>`: the shared one carries `.t-button`,
          whose unlayered font-size beats a size utility on the element. */}
      <button
        type='button'
        onClick={() => setConfirming(true)}
        className='inline-flex shrink-0 items-center gap-1 px-1.5 py-0.5 text-muted-foreground text-xs outline-none transition-colors hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 disabled:opacity-50'
        data-testid={`admin-airhouse-provision-${row.workspace_id}`}
      >
        <Warehouse className='size-3' />
        Provision
      </button>
      <ProvisionConfirmDialog
        workspaceId={row.workspace_id}
        workspaceName={row.workspace_name}
        orgName={row.org_name}
        open={confirming}
        onOpenChange={setConfirming}
      />
    </div>
  );
};
