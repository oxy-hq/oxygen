import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

/**
 * The one-line pointer shown in place of an action that used to write the
 * workspace's files from the product: the change belongs in the workspace's
 * repository, or, with no repository, in Settings first. The same pointer the
 * "Work on this workspace in your editor" panel gives.
 */
export default function WorkspaceRepositoryNotice({ className }: { className?: string }) {
  const { workspace } = useCurrentWorkspace();
  const hasRemote = !!workspace?.git_remote_url;
  return (
    <p
      className={className ?? "text-muted-foreground text-sm"}
      data-testid='workspace-repository-notice'
    >
      {hasRemote
        ? "Changed in the workspace's repository now: clone it (Files, in the Factory sidebar, shows how) and push to the default branch; it compiles and deploys."
        : "Changed in the workspace's repository now, and this workspace has none: connect one in Settings first."}
    </p>
  );
}
