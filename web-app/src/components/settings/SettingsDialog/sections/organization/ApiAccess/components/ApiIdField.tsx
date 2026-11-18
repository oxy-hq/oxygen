import { Copy } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

/**
 * The id most API paths still take (`/api/<id>/agents`). It is the org's workspace id: the
 * server keys those routes by workspace, and each org has exactly one, so to a person it is
 * simply "the id for API calls". It moved here from Workspace → API tokens when that page went;
 * until the API takes the organization instead, this is the one place to copy it from.
 */
export function ApiIdField() {
  const { workspace } = useCurrentWorkspace();
  if (!workspace?.id) return null;

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(workspace.id);
      toast.success("Copied to clipboard");
    } catch {
      toast.error("Failed to copy to clipboard");
    }
  };

  return (
    <div className='space-y-2' data-testid='api-tokens-api-id'>
      <p className='text-muted-foreground text-xs'>
        ID for API calls: the <code>{"{id}"}</code> in <code>/api/{"{id}"}/…</code>
      </p>
      <div className='flex items-center gap-2'>
        <div className='flex h-8 flex-1 items-center rounded-md border bg-background px-3 font-mono text-xs'>
          {workspace.id}
        </div>
        <Button variant='outline' size='sm' onClick={copy} aria-label='Copy the ID for API calls'>
          <Copy className='h-4 w-4' />
        </Button>
      </div>
    </div>
  );
}
