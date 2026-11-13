import { ArrowUp, MessageCircleQuestion, Play } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { HighlightTextarea } from "@/components/ui/HighlightTextarea";
import { Button } from "@/components/ui/shadcn/button";
import { Select, SelectContent, SelectTrigger, SelectValue } from "@/components/ui/shadcn/select";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useThreadMutation from "@/hooks/api/threads/useThreadMutation";
import useRunAutomationThread from "@/hooks/automation/useRunAutomationThread";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { useEnterSubmit } from "@/hooks/useEnterSubmit";
import ROUTES from "@/libs/utils/routes";
import { getShortTitle } from "@/libs/utils/string";
import type { ThinkingMode } from "@/services/api/analytics";
import { setPendingThinkingMode } from "@/stores/analyticsThinkingMode";
import useCurrentOrg from "@/stores/useCurrentOrg";
import AgentsDropdown, { type Agent } from "./AgentsDropdown";
import AutomationsDropdown, { type AutomationOption } from "./AutomationsDropdown";
import SelectItemWithDetail from "./SelectItemWithDetail";
import ThinkingModeMenu from "./ThinkingModeMenu";
import { resolveDefaultAgent, useAgentOptions } from "./useAgentOptions";

const ChatPanel = ({
  initialMessage,
  initialAgentPath,
  autoSubmit,
  onThreadCreated,
  placeholderOverride,
  lockMode,
  hideAgentPicker
}: {
  initialMessage?: string;
  initialAgentPath?: string;
  autoSubmit?: boolean;
  /** When set, called with the new thread id instead of navigating to it. */
  onThreadCreated?: (threadId: string) => void;
  /** Overrides the mode-derived placeholder (e.g. a branded Ask prompt). */
  placeholderOverride?: string;
  /** Pins the composer to a single mode and hides the mode selector
   *  (the Ask surfaces only ever ask). */
  lockMode?: "ask";
  /** Hides the agent picker and auto-selects the workspace default agent.
   *  Extended Thinking stays available via the inline ThinkingModeMenu. */
  hideAgentPicker?: boolean;
}) => {
  const navigate = useNavigate();
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";

  const { run: runAutomation } = useRunAutomationThread();

  const [agent, setAgent] = useState<Agent | null>(null);
  const [automation, setAutomation] = useState<AutomationOption | null>(null);

  const { mutate: createThread, isPending } = useThreadMutation((data) => {
    switch (data.source_type) {
      case "analytics":
        // Run creation is handled by AnalyticsThread's auto-start on first visit.
        // Do NOT create a run here — it races with auto-start and causes duplicates.
        setPendingThinkingMode(data.id, thinkingMode);
        break;
      case "workflow":
        // `data.source` carries the automation's path-base64 (set when the
        // thread was created above). The new agentic-automations runner
        // decodes it back into a workflow_ref. Cannot reject: the run catches
        // its own start/stream failures and reports them with a toast.
        void runAutomation(data.id, data.source);
        break;
    }
    // Clear the composer once the thread exists. On the onThreadCreated
    // path the panel stays mounted, so a lingering message could be
    // re-submitted and create a duplicate thread.
    setMessage("");
    if (onThreadCreated) {
      onThreadCreated(data.id);
    } else {
      navigate(ROUTES.ORG(orgSlug).WORKSPACE(projectId).THREAD(data.id));
    }
  });

  const autoSubmitDone = useRef(false);

  const [message, setMessage] = useState(initialMessage ?? "");

  // Auto-submit when navigated with prefilled question + agent (e.g. from onboarding)
  // biome-ignore lint/correctness/useExhaustiveDependencies: formRef is a stable ref
  useEffect(() => {
    if (autoSubmit && !autoSubmitDone.current && message && agent && !isPending) {
      autoSubmitDone.current = true;
      formRef.current?.requestSubmit();
    }
  }, [autoSubmit, message, agent, isPending]);

  const { formRef, onKeyDown: enterSubmitKeyDown } = useEnterSubmit();
  const [mode, setMode] = useState<string>(lockMode ?? "ask");
  const [thinkingMode, setThinkingMode] = useState<ThinkingMode>("auto");

  // When the agent picker is hidden, resolve and lock in the workspace
  // default agent (the analytics .agentic.yml, else the first agent) so
  // submit + auto-submit have an agent without any user choice.
  const { agentOptions, isSuccess: agentsLoaded } = useAgentOptions();
  useEffect(() => {
    if (!hideAgentPicker || agent || !agentsLoaded) return;
    const resolved = resolveDefaultAgent(agentOptions, initialAgentPath);
    if (resolved) {
      if (!resolved.isAnalytics) setThinkingMode("auto");
      setAgent(resolved);
    }
  }, [hideAgentPicker, agent, agentsLoaded, agentOptions, initialAgentPath]);

  const handleTextareaKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    enterSubmitKeyDown(e);
  };

  const handleTextareaChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setMessage(e.target.value);
  };

  const handleFormSubmit = async (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    if (isPending) return;
    const input = message;
    const title = getShortTitle(message);

    switch (mode) {
      case "ask":
        if (!agent) return;
        createThread({
          title: title,
          source: agent.id,
          source_type: agent.isAnalytics ? "analytics" : "agent",
          input
        });
        break;
      case "workflow":
        if (!automation) return;
        createThread({
          title: title ? title : automation.name,
          source: automation.id,
          source_type: "workflow",
          input: message
        });
        break;
    }
  };

  const submitIcon = mode === "workflow" ? <Play /> : <ArrowUp />;
  const disabled = () => {
    if (isPending) return true;
    switch (mode) {
      case "ask":
        return !message || !agent;
      case "workflow":
        return !automation;
    }
  };

  const placeholder = (() => {
    if (placeholderOverride && mode === "ask") return placeholderOverride;
    switch (mode) {
      case "ask":
        return "Start your request, and let Oxygen handle everything.";
      case "workflow":
        return "Enter a title for this automation run.";
    }
  })();

  return (
    <form
      ref={formRef}
      onSubmit={handleFormSubmit}
      className='relative mx-auto flex w-full max-w-[672px] flex-col gap-1 rounded-md border bg-background p-2'
    >
      <HighlightTextarea
        disabled={isPending}
        name='question'
        autoFocus
        onKeyDown={handleTextareaKeyDown}
        value={message}
        onChange={handleTextareaChange}
        placeholder={placeholder}
        overlayClassName='px-0 py-2 text-base md:text-sm'
        className='customScrollbar max-h-[200px] resize-none border-none bg-transparent px-0 text-base shadow-none outline-none placeholder:text-base hover:border-none focus-visible:border-none focus-visible:shadow-none focus-visible:ring-0 focus-visible:ring-offset-0 md:text-sm md:placeholder:text-sm'
      />

      <div className='flex flex-wrap items-center justify-between gap-2'>
        {!lockMode && (
          <div className='flex items-center justify-center'>
            <Select value={mode} onValueChange={setMode}>
              <SelectTrigger size='sm'>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItemWithDetail
                  className='cursor-pointer'
                  value='ask'
                  detail={{
                    title: "Ask",
                    description:
                      "Interact in natural language to get instant insights. No SQL or technical knowledge required."
                  }}
                >
                  <MessageCircleQuestion className='size-4' />
                  Ask
                </SelectItemWithDetail>
                <SelectItemWithDetail
                  className='cursor-pointer'
                  value='workflow'
                  detail={{
                    title: "Automation",
                    description:
                      "Automate multi-step processes with intelligent agents that execute complex automations autonomously."
                  }}
                >
                  <Play className='size-4' />
                  Automation
                </SelectItemWithDetail>
              </SelectContent>
            </Select>
          </div>
        )}
        <div className='flex flex-1 items-center justify-end gap-2'>
          {mode === "ask" &&
            (hideAgentPicker ? (
              agent?.isAnalytics && (
                <ThinkingModeMenu
                  value={thinkingMode}
                  onChange={setThinkingMode}
                  disabled={isPending}
                />
              )
            ) : (
              <AgentsDropdown
                onSelect={(a) => {
                  if (!a.isAnalytics) setThinkingMode("auto");
                  setAgent(a);
                }}
                agentSelected={agent}
                preferAgentPath={initialAgentPath}
                thinkingMode={thinkingMode}
                onThinkingModeChange={setThinkingMode}
                disabled={isPending}
              />
            ))}
          {mode === "workflow" && (
            <AutomationsDropdown onSelect={setAutomation} automation={automation} />
          )}
          <Button
            size='sm'
            disabled={disabled()}
            type='submit'
            data-testid='chat-panel-submit-button'
          >
            {isPending ? <Spinner /> : submitIcon}
          </Button>
        </div>
      </div>
    </form>
  );
};

export default ChatPanel;
