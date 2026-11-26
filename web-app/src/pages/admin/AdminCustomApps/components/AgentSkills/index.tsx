import { Bot, X } from "lucide-react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/shadcn/popover";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { CommandSnippet } from "@/pages/admin/components/CommandSnippet";
import { AgentSkillsInstall } from "./AgentSkillsInstall";
import { QUICK_INSTALL } from "./installMethods";

export { useAgentSkillsDismissed } from "./useAgentSkillsDismissed";

/**
 * The reminder to give a coding agent the Oxy skills.
 *
 * Most custom apps are now written by an agent, and an agent without the skills guesses at
 * the app contract — the base path, the function manifest, what a sandbox token may do.
 * So the fleet leads with one line and the command that works for any agent, until it is
 * dismissed; after that the same instructions stay one click away in the header, because a
 * reminder that cannot be found again is just a banner that went away.
 */
export const AgentSkillsStrip = ({ onDismiss }: { onDismiss: () => void }) => (
  <div
    data-testid='admin-agent-skills-strip'
    className={cn(
      "flex flex-wrap items-center gap-x-2 gap-y-1 border-b px-4 py-1.5 text-xs",
      ADMIN_TONE.info.bg
    )}
  >
    <Bot className='size-3 shrink-0 text-muted-foreground' aria-hidden />
    <span className='font-medium'>Building an app with a coding agent?</span>
    <span className='text-muted-foreground'>
      Install the Oxy skills so it knows the app contract, Oxy Functions and how to publish.
    </span>
    <CommandSnippet command={QUICK_INSTALL} className='w-60' />
    <Popover>
      <PopoverTrigger
        data-testid='admin-agent-skills-other-ways'
        className='text-muted-foreground underline decoration-dotted underline-offset-2 hover:text-foreground'
      >
        Other ways
      </PopoverTrigger>
      <PopoverContent align='start' className={`${DESIGN_REFERENCE} w-96 p-0`}>
        <AgentSkillsInstall />
      </PopoverContent>
    </Popover>
    <Button
      variant='ghost'
      size='icon'
      className='ml-auto size-6 text-muted-foreground'
      onClick={onDismiss}
      aria-label='Dismiss the agent skills reminder'
      data-testid='admin-agent-skills-dismiss'
    >
      <X className='size-3' />
    </Button>
  </div>
);

/** The same instructions, one click away, once the strip is gone or on an app's console. */
export const AgentSkillsButton = () => (
  <Popover>
    <PopoverTrigger asChild>
      <Button
        variant='ghost'
        size='sm'
        className='h-7 gap-1 text-xs'
        data-testid='admin-agent-skills-button'
      >
        <Bot className='size-3' aria-hidden />
        Agent skills
      </Button>
    </PopoverTrigger>
    <PopoverContent align='end' className={`${DESIGN_REFERENCE} w-96 p-0`}>
      <AgentSkillsInstall />
    </PopoverContent>
  </Popover>
);
