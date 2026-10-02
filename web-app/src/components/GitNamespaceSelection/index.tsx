import { X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { Label } from "@/components/ui/shadcn/label";
import { useDeleteGitNamespace, useGitHubNamespaces } from "@/hooks/api/github";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { GitHubNamespace } from "@/types/github";
import AddGitNamespaceFlow from "./AddGitNamespaceFlow";
import NamespaceList from "./NamespaceList";

interface Props {
  value?: string;
  onChange?: (value: string) => void;
}

export const GitNamespaceSelection = ({ value, onChange }: Props) => {
  const { org } = useCurrentOrg();
  const orgId = org?.id ?? "";

  const {
    data: gitNamespaces = [],
    isPending: isLoadingNamespaces,
    refetch
  } = useGitHubNamespaces(orgId);
  const { mutate: deleteNamespace } = useDeleteGitNamespace();

  const [addOpen, setAddOpen] = useState(false);

  // Snapshot the namespace count when the add flow opens so we can detect when
  // a new namespace is added (e.g. via popup on a different domain).
  const namespaceCountOnOpenRef = useRef(0);
  useEffect(() => {
    if (addOpen) namespaceCountOnOpenRef.current = gitNamespaces.length;
  }, [addOpen, gitNamespaces.length]);

  // When a new namespace appears while the add flow is open, treat it as a
  // successful connection and close the flow automatically.
  useEffect(() => {
    if (!addOpen || gitNamespaces.length <= namespaceCountOnOpenRef.current) return;
    const newest = gitNamespaces[gitNamespaces.length - 1];
    setAddOpen(false);
    onChange?.(newest.id);
  }, [addOpen, gitNamespaces, onChange]);

  // Auto-select the only connected namespace so the user doesn't have to click it.
  useEffect(() => {
    if (!isLoadingNamespaces && gitNamespaces.length === 1 && !value) {
      onChange?.(gitNamespaces[0].id);
    }
  }, [isLoadingNamespaces, gitNamespaces, value, onChange]);

  const handleConnected = (namespaceId: string) => {
    setAddOpen(false);
    // refetch settles into the query's error state; it does not reject.
    void refetch().then(() => onChange?.(namespaceId));
  };

  const handleDelete = (ns: GitHubNamespace) => {
    if (ns.id === value) onChange?.("");
    deleteNamespace({ orgId, id: ns.id });
  };

  return (
    <div className='space-y-2'>
      <Label>GitHub account</Label>

      <NamespaceList
        namespaces={gitNamespaces}
        isLoading={isLoadingNamespaces}
        value={value}
        onChange={onChange}
        onDelete={handleDelete}
        onAdd={() => setAddOpen(true)}
      />

      {value && (
        <button
          type='button'
          onClick={() => onChange?.("")}
          className='flex items-center gap-1 text-muted-foreground text-xs hover:text-foreground'
        >
          <X className='h-3 w-3' />
          Clear selection
        </button>
      )}

      <AddGitNamespaceFlow
        orgId={orgId}
        open={addOpen}
        onOpenChange={setAddOpen}
        onConnected={handleConnected}
      />
    </div>
  );
};
