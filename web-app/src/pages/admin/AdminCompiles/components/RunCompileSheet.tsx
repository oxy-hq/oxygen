import { Loader2, PlayCircle } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import {
  Sheet,
  SheetClose,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
  SheetTrigger
} from "@/components/ui/shadcn/sheet";
import { useRunCompileNow } from "@/hooks/api/compiles";

/**
 * "Run compile now" moved off the page into a header Sheet to reclaim
 * vertical space. Keeps every field of the original inline form (workspace
 * id, git sha, branch, promote) — enqueues a `Compile` TaskSpec for ad-hoc
 * investigation. An optional `defaultWorkspaceId` lets a row prefill the
 * workspace when re-running a specific tenant.
 */
export const RunCompileSheet = ({ defaultWorkspaceId = "" }: { defaultWorkspaceId?: string }) => {
  const [open, setOpen] = useState(false);
  const [workspaceId, setWorkspaceId] = useState(defaultWorkspaceId);
  const [gitSha, setGitSha] = useState("");
  const [branch, setBranch] = useState("");
  const [promote, setPromote] = useState(false);
  const mutation = useRunCompileNow();

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = workspaceId.trim();
    if (!trimmed) {
      toast.error("Workspace ID is required");
      return;
    }
    mutation.mutate(
      {
        workspace_id: trimmed,
        git_sha: gitSha.trim() || undefined,
        branch: branch.trim() || undefined,
        promote
      },
      {
        onSuccess: (res) => {
          toast.success(`Enqueued compile task ${res.task_id.slice(0, 8)}…`);
          setGitSha("");
          setBranch("");
          setOpen(false);
        },
        onError: (err) => {
          toast.error(err instanceof Error ? err.message : "Enqueue failed");
        }
      }
    );
  };

  return (
    <Sheet open={open} onOpenChange={setOpen}>
      <SheetTrigger asChild>
        <Button size='sm' className='h-8 gap-1.5'>
          <PlayCircle className='size-3.5' />
          Run compile
        </Button>
      </SheetTrigger>
      <SheetContent className={`${DESIGN_REFERENCE} w-full gap-0 sm:max-w-md`}>
        <SheetHeader>
          <SheetTitle>Run compile now</SheetTitle>
          <SheetDescription>
            Enqueue an ad-hoc compile for a single workspace. GitHub pushes do this automatically;
            use this when investigating an incident.
          </SheetDescription>
        </SheetHeader>
        <form onSubmit={submit} className='flex min-h-0 flex-1 flex-col gap-4 px-4'>
          <div className='space-y-1.5'>
            <Label htmlFor='run-workspace-id' className='text-xs'>
              Workspace ID <span className='text-destructive'>*</span>
            </Label>
            <Input
              id='run-workspace-id'
              placeholder='00000000-0000-0000-0000-000000000000'
              value={workspaceId}
              onChange={(e) => setWorkspaceId(e.target.value)}
              className='h-8 font-mono text-xs'
              required
            />
          </div>
          <div className='space-y-1.5'>
            <Label htmlFor='run-git-sha' className='text-xs'>
              Git SHA
            </Label>
            <Input
              id='run-git-sha'
              placeholder='auto (resolves HEAD)'
              value={gitSha}
              onChange={(e) => setGitSha(e.target.value)}
              className='h-8 font-mono text-xs'
            />
          </div>
          <div className='space-y-1.5'>
            <Label htmlFor='run-branch' className='text-xs'>
              Branch
            </Label>
            <Input
              id='run-branch'
              placeholder='main'
              value={branch}
              onChange={(e) => setBranch(e.target.value)}
              className='h-8 text-xs'
            />
          </div>
          <div className='flex items-start gap-2 border border-border/60 bg-muted/30 p-3 text-xs'>
            <Checkbox
              id='run-promote'
              checked={promote}
              onCheckedChange={(v) => setPromote(v === true)}
              className='mt-0.5'
            />
            <Label
              htmlFor='run-promote'
              className='cursor-pointer font-normal text-muted-foreground leading-snug'
            >
              Promote the resulting revision into{" "}
              <code className='font-mono text-foreground'>workspaces.current_revision_id</code>
            </Label>
          </div>
          <SheetFooter className='mt-auto flex-row justify-end gap-2 px-0'>
            <SheetClose asChild>
              <Button type='button' variant='outline' size='sm' className='h-8'>
                Cancel
              </Button>
            </SheetClose>
            <Button type='submit' size='sm' disabled={mutation.isPending} className='h-8'>
              {mutation.isPending ? <Loader2 className='mr-1.5 size-3 animate-spin' /> : null}
              Enqueue
            </Button>
          </SheetFooter>
        </form>
      </SheetContent>
    </Sheet>
  );
};
