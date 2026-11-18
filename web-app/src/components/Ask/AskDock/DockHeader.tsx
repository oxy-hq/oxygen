import { ExternalLink, History, PanelRightClose, Plus } from "lucide-react";
import { useNavigate } from "react-router-dom";
import { OxyMark } from "@/components/OxyMark";
import { Button } from "@/components/ui/shadcn/button";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import ROUTES from "@/libs/utils/routes";
import useAskDock from "@/stores/useAskDock";
import useCurrentOrg from "@/stores/useCurrentOrg";

const titles: Record<string, string> = {
  composer: "Ask Oxygen",
  thread: "Thread",
  history: "Recent threads"
};

/** The dock's top strip: brand mark + contextual title + controls
 *  (full view · new · history · collapse). Collapse preserves all state —
 *  the top-bar Ask Oxygen button re-opens it. */
export function DockHeader() {
  const view = useAskDock((s) => s.view);
  const threadId = useAskDock((s) => s.threadId);
  const showHistory = useAskDock((s) => s.showHistory);
  const newChat = useAskDock((s) => s.newChat);
  const close = useAskDock((s) => s.close);
  const navigate = useNavigate();
  const { project } = useCurrentProjectBranch();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";

  const openFull = () => {
    if (!threadId || !project?.id) return;
    const to = ROUTES.ORG(orgSlug).WORKSPACE().THREAD(threadId);
    close();
    navigate(to);
  };

  return (
    <div className='flex shrink-0 items-center gap-1 border-b py-2 pr-2 pl-3'>
      <OxyMark className='size-4 shrink-0 text-primary' />
      <span className='flex-1 truncate font-medium text-sm'>{titles[view]}</span>
      {view === "thread" && (
        <Button
          variant='ghost'
          size='icon'
          onClick={openFull}
          aria-label='Open full view'
          data-testid='ask-dock-full'
          tooltip={{ content: "Full view", side: "bottom" }}
          className='size-8 text-muted-foreground hover:text-foreground'
        >
          <ExternalLink className='size-4' />
        </Button>
      )}
      {view !== "composer" && (
        <Button
          variant='ghost'
          size='icon'
          onClick={newChat}
          aria-label='New chat'
          data-testid='ask-dock-new'
          tooltip={{ content: "New chat", side: "bottom" }}
          className='size-8 text-muted-foreground hover:text-foreground'
        >
          <Plus className='size-4' />
        </Button>
      )}
      {view !== "history" && (
        <Button
          variant='ghost'
          size='icon'
          onClick={showHistory}
          aria-label='Recent threads'
          data-testid='ask-dock-history'
          tooltip={{ content: "Recent threads", side: "bottom" }}
          className='size-8 text-muted-foreground hover:text-foreground'
        >
          <History className='size-4' />
        </Button>
      )}
      <Button
        variant='ghost'
        size='icon'
        onClick={close}
        aria-label='Collapse Ask'
        data-testid='ask-dock-collapse'
        tooltip={{ content: "Collapse", side: "bottom" }}
        className='size-8 text-muted-foreground hover:text-foreground'
      >
        <PanelRightClose className='size-4' />
      </Button>
    </div>
  );
}
