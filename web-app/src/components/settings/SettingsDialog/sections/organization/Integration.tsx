import { useQueryClient } from "@tanstack/react-query";
import { CheckCircle2, Plus, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import AddGitNamespaceFlow from "@/components/GitNamespaceSelection/AddGitNamespaceFlow";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger
} from "@/components/ui/shadcn/alert-dialog";
import { Button } from "@/components/ui/shadcn/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import { useDeleteGitNamespace } from "@/hooks/api/github/useDeleteGitNamespace";
import { useGitHubNamespaces } from "@/hooks/api/github/useGitHubNamespaces";
import queryKeys from "@/hooks/api/queryKey";
import { useSlackDisconnect } from "@/hooks/api/slack/useSlackDisconnect";
import { useSlackInstallation } from "@/hooks/api/slack/useSlackInstallation";
import { SlackService } from "@/services/api/slack";
import type { Organization } from "@/types/organization";

const GithubIcon = ({ className }: { className?: string }) => (
  <svg className={className} viewBox='0 0 24 24' fill='currentColor' aria-hidden='true'>
    <path d='M12 2C6.477 2 2 6.477 2 12c0 4.418 2.865 8.166 6.839 9.489.5.092.682-.217.682-.482 0-.237-.009-.868-.013-1.703-2.782.603-3.369-1.342-3.369-1.342-.454-1.154-1.11-1.462-1.11-1.462-.908-.62.069-.608.069-.608 1.003.07 1.532 1.031 1.532 1.031.891 1.528 2.341 1.087 2.91.831.091-.645.349-1.087.635-1.337-2.22-.252-4.555-1.11-4.555-4.943 0-1.091.39-1.984 1.03-2.682-.103-.253-.447-1.27.097-2.646 0 0 .84-.269 2.75 1.025A9.578 9.578 0 0112 6.836a9.59 9.59 0 012.504.337c1.909-1.294 2.748-1.025 2.748-1.025.546 1.376.202 2.394.1 2.646.64.699 1.026 1.591 1.026 2.682 0 3.841-2.337 4.687-4.565 4.935.359.309.678.919.678 1.852 0 1.337-.012 2.414-.012 2.742 0 .267.18.578.688.48C19.138 20.163 22 16.418 22 12c0-5.523-4.477-10-10-10z' />
  </svg>
);

// Full four-colour Slack logo — each arm segment gets its official brand colour.
const SlackIcon = ({ size = 20 }: { size?: number }) => (
  <svg width={size} height={size} viewBox='0 0 24 24' aria-hidden='true'>
    {/* Green — top-left vertical bar */}
    <path
      fill='#36C5F0'
      d='M5.042 15.165a2.528 2.528 0 0 1-2.52 2.523A2.528 2.528 0 0 1 0 15.165a2.527 2.527 0 0 1 2.522-2.52h2.52v2.52z'
    />
    <path
      fill='#36C5F0'
      d='M6.313 15.165a2.527 2.527 0 0 1 2.521-2.52 2.527 2.527 0 0 1 2.521 2.52v6.313A2.528 2.528 0 0 1 8.834 24a2.528 2.528 0 0 1-2.521-2.522v-6.313z'
    />
    {/* Yellow — top-right horizontal bar */}
    <path
      fill='#2EB67D'
      d='M8.834 5.042a2.528 2.528 0 0 1-2.521-2.52A2.528 2.528 0 0 1 8.834 0a2.528 2.528 0 0 1 2.521 2.522v2.52H8.834z'
    />
    <path
      fill='#2EB67D'
      d='M8.834 6.313a2.528 2.528 0 0 1 2.521 2.521 2.528 2.528 0 0 1-2.521 2.521H2.522A2.528 2.528 0 0 1 0 8.834a2.528 2.528 0 0 1 2.522-2.521h6.312z'
    />
    {/* Red — bottom-right vertical bar */}
    <path
      fill='#ECB22E'
      d='M18.956 8.834a2.528 2.528 0 0 1 2.522-2.521A2.528 2.528 0 0 1 24 8.834a2.528 2.528 0 0 1-2.522 2.521h-2.522V8.834z'
    />
    <path
      fill='#ECB22E'
      d='M17.688 8.834a2.528 2.528 0 0 1-2.523 2.521 2.527 2.527 0 0 1-2.52-2.521V2.522A2.527 2.527 0 0 1 15.165 0a2.528 2.528 0 0 1 2.523 2.522v6.312z'
    />
    {/* Blue — bottom-left horizontal bar */}
    <path
      fill='#E01E5A'
      d='M15.165 18.956a2.528 2.528 0 0 1 2.523 2.522A2.528 2.528 0 0 1 15.165 24a2.527 2.527 0 0 1-2.52-2.522v-2.522h2.52z'
    />
    <path
      fill='#E01E5A'
      d='M15.165 17.688a2.527 2.527 0 0 1-2.52-2.523 2.526 2.526 0 0 1 2.52-2.52h6.313A2.527 2.527 0 0 1 24 15.165a2.528 2.528 0 0 1-2.522 2.523h-6.313z'
    />
  </svg>
);

interface IntegrationSectionProps {
  org: Organization;
}

function GitHubConnections({ org }: IntegrationSectionProps) {
  const { data: namespaces, isLoading } = useGitHubNamespaces(org.id);
  const deleteNamespace = useDeleteGitNamespace();
  const canManage = org.role === "owner" || org.role === "admin";
  const [addOpen, setAddOpen] = useState(false);

  const handleDelete = async (id: string, name: string) => {
    try {
      await deleteNamespace.mutateAsync({ orgId: org.id, id });
      toast.success(`${name} disconnected`);
    } catch {
      toast.error("Failed to disconnect GitHub connection");
    }
  };

  const handleConnected = () => {
    setAddOpen(false);
    toast.success("GitHub connection added");
  };

  return (
    <div className='space-y-3'>
      <div className='flex items-start justify-between gap-4'>
        <div className='space-y-1'>
          <h3 className='font-medium'>GitHub Connections</h3>
          <p className='text-muted-foreground text-sm'>
            GitHub App installations connected to this organization.
          </p>
        </div>
        {canManage && namespaces && namespaces.length > 0 && (
          <Button size='sm' className='gap-1.5' onClick={() => setAddOpen(true)}>
            <Plus className='h-4 w-4' />
            Add connection
          </Button>
        )}
      </div>

      {isLoading ? (
        <div className='py-8 text-center text-muted-foreground text-sm'>Loading...</div>
      ) : namespaces && namespaces.length > 0 ? (
        <div className='divide-y divide-border rounded-lg border border-border'>
          {namespaces.map((ns) => (
            <div key={ns.id} className='flex items-center gap-3 px-4 py-3'>
              <div className='flex h-8 w-8 items-center justify-center rounded-full bg-foreground'>
                <GithubIcon className='h-4 w-4 text-background' />
              </div>
              <div className='min-w-0 flex-1'>
                <div className='truncate font-medium text-sm'>{ns.name}</div>
                <div className='truncate text-muted-foreground text-xs capitalize'>
                  {ns.owner_type} &middot; {ns.slug}
                </div>
              </div>
              {canManage && (
                <AlertDialog>
                  <AlertDialogTrigger asChild>
                    <Button
                      variant='ghost'
                      size='icon'
                      disabled={deleteNamespace.isPending}
                      aria-label={`Disconnect ${ns.name}`}
                    >
                      <Trash2 className='h-4 w-4 text-muted-foreground' />
                    </Button>
                  </AlertDialogTrigger>
                  <AlertDialogContent>
                    <AlertDialogHeader>
                      <AlertDialogTitle>Disconnect "{ns.name}"?</AlertDialogTitle>
                      <AlertDialogDescription>
                        This will remove the GitHub connection for this organization. Workspaces
                        linked to repositories under this connection will lose access until it is
                        re-added.
                      </AlertDialogDescription>
                    </AlertDialogHeader>
                    <AlertDialogFooter>
                      <AlertDialogCancel>Cancel</AlertDialogCancel>
                      <AlertDialogAction
                        className='bg-destructive text-destructive-foreground hover:bg-destructive/90'
                        onClick={() => handleDelete(ns.id, ns.name)}
                      >
                        Disconnect
                      </AlertDialogAction>
                    </AlertDialogFooter>
                  </AlertDialogContent>
                </AlertDialog>
              )}
            </div>
          ))}
        </div>
      ) : (
        <div className='flex flex-col items-center justify-center gap-3 rounded-lg border border-border border-dashed py-10 text-center'>
          <div className='flex h-10 w-10 items-center justify-center rounded-full bg-foreground'>
            <GithubIcon className='h-5 w-5 text-background' />
          </div>
          <p className='text-muted-foreground text-sm'>No GitHub connections yet.</p>
          {canManage && (
            <Button size='sm' className='gap-1.5' onClick={() => setAddOpen(true)}>
              <Plus className='h-4 w-4' />
              Connect GitHub
            </Button>
          )}
        </div>
      )}

      <AddGitNamespaceFlow
        orgId={org.id}
        canUseApp={canManage}
        open={addOpen}
        onOpenChange={setAddOpen}
        onConnected={handleConnected}
      />
    </div>
  );
}

function SlackConnection({ org }: IntegrationSectionProps) {
  const { data, isLoading } = useSlackInstallation(org.id);
  const disconnect = useSlackDisconnect(org.id);
  const queryClient = useQueryClient();
  const canManage = org.role === "owner" || org.role === "admin";
  const [isInstalling, setIsInstalling] = useState(false);

  // After kicking off OAuth in a new tab, refresh the installation status
  // every time the original tab regains focus. The user authorizes Slack
  // in the new tab, switches back to Oxygen, and the dialog flips to
  // "Connected" without a manual refresh.
  useEffect(() => {
    if (!isInstalling) return;
    const onFocus = () => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.slack.installation(org.id)
      });
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [isInstalling, org.id, queryClient]);

  // Once the installation flips to connected, stop listening. The inline
  // "Connected" badge auto-flips here; the success toast is fired by the
  // OAuth-tab landing (`WorkspaceLayout` in App.tsx) after `?slack_installed=ok`,
  // so re-firing it here would double-toast users who keep both tabs open.
  useEffect(() => {
    if (data?.connected && isInstalling) {
      setIsInstalling(false);
    }
  }, [data?.connected, isInstalling]);

  const handleInstall = async () => {
    try {
      const opened = await SlackService.startInstall(org.id);
      if (!opened) {
        toast.error("Allow popups for this site to install Slack.");
        return;
      }
      setIsInstalling(true);
    } catch {
      toast.error("Failed to start Slack install");
    }
  };

  const handleDisconnect = async () => {
    try {
      await disconnect.mutateAsync();
      toast.success("Slack disconnected");
    } catch {
      toast.error("Failed to disconnect Slack");
    }
  };

  return (
    <div className='space-y-3'>
      <div className='space-y-1'>
        <h3 className='font-medium'>Slack</h3>
        <p className='text-muted-foreground text-sm'>
          Query Oxygen from Slack with @mentions, DMs, and App Home.
        </p>
      </div>

      <div className='rounded-lg border border-border p-4'>
        <div className='flex items-center justify-between gap-4'>
          <div className='flex items-center gap-3'>
            <div className='flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-background shadow-sm ring-1 ring-border'>
              <SlackIcon size={20} />
            </div>
            <div className='min-w-0'>
              {data?.connected ? (
                // Slack workspace name lives in the tooltip — only one
                // Slack workspace per Oxygen workspace, so showing it
                // inline is redundant; the tooltip is enough for users
                // who actually want to know which workspace it is.
                <Tooltip>
                  <TooltipTrigger asChild>
                    <div className='flex cursor-default items-center gap-2'>
                      <CheckCircle2 className='h-3.5 w-3.5 shrink-0 text-primary' />
                      <p className='font-medium text-sm decoration-muted-foreground/30 decoration-dotted underline-offset-4 hover:underline'>
                        Connected
                      </p>
                    </div>
                  </TooltipTrigger>
                  <TooltipContent>Workspace: {data.team_name ?? "Slack workspace"}</TooltipContent>
                </Tooltip>
              ) : (
                <>
                  <p className='font-medium text-sm'>Not connected</p>
                  <p className='mt-0.5 text-muted-foreground text-xs'>
                    Install the Oxygen Slack app to start querying from Slack.
                  </p>
                </>
              )}
            </div>
          </div>

          {canManage && (
            <div className='flex shrink-0 items-center gap-2'>
              {isLoading ? null : data?.connected ? (
                <>
                  <Button variant='outline' size='sm' onClick={handleInstall}>
                    Reinstall
                  </Button>
                  <Button
                    variant='destructive'
                    size='sm'
                    disabled={disconnect.isPending}
                    onClick={handleDisconnect}
                  >
                    {disconnect.isPending ? "Disconnecting..." : "Disconnect"}
                  </Button>
                </>
              ) : (
                <Button size='sm' onClick={handleInstall}>
                  Install to Slack
                </Button>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

export default function IntegrationSection({ org }: IntegrationSectionProps) {
  return (
    <div className='space-y-8'>
      <GitHubConnections org={org} />
      <SlackConnection org={org} />
    </div>
  );
}
