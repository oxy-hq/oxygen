import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

/**
 * The one-line pointer shown in place of an action whose route the server
 * refuses once the IDE is retired (`ide_retired`): the change belongs in the
 * workspace's repository, or, with no repository, in Settings first. The same
 * pointer the "Work on this workspace in your editor" panel gives.
 */
export default function RetiredIdeNotice({ className }: { className?: string }) {
  const { workspace } = useCurrentWorkspace();
  const hasRemote = !!workspace?.git_remote_url;
  return (
    <p className={className ?? "text-muted-foreground text-sm"} data-testid='retired-ide-notice'>
      {hasRemote
        ? "Changed in the workspace's repository now: clone it (Work in your editor, in the Factory sidebar) and push to the default branch; it compiles and deploys."
        : "Changed in the workspace's repository now, and this workspace has none: connect one in Settings first."}
    </p>
  );
}
