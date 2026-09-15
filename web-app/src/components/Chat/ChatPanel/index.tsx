import { ArrowUp, Hammer, MessageCircleQuestion, Play, Zap } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { HighlightTextarea } from "@/components/ui/HighlightTextarea";
import { Button } from "@/components/ui/shadcn/button";
import { Select, SelectContent, SelectTrigger, SelectValue } from "@/components/ui/shadcn/select";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useFileTree from "@/hooks/api/files/useFileTree";
import useThreadMutation from "@/hooks/api/threads/useThreadMutation";
import useBuilderAvailable from "@/hooks/api/useBuilderAvailable";
import useRunAutomationThread from "@/hooks/automation/useRunAutomationThread";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { useEnterSubmit } from "@/hooks/useEnterSubmit";
import { useMentionHighlight } from "@/hooks/useMentionHighlight";
import { cn } from "@/libs/shadcn/utils";
import { flattenFiles, getActiveMention, getCleanObjectName } from "@/libs/utils/mention";
import ROUTES from "@/libs/utils/routes";
import { getShortTitle } from "@/libs/utils/string";
import { getFileTypeIcon } from "@/pages/ide/Files/FilesSidebar/utils";
import type { ThinkingMode } from "@/services/api/analytics";
import { setPendingThinkingMode } from "@/stores/analyticsThinkingMode";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { FileTreeModel } from "@/types/file";
import { detectFileType } from "@/utils/fileTypes";
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

  const {
    isAvailable: isBuilderAvailable,
    isLoading: isCheckingBuilder,
    isBuiltin,
    builderPath
  } = useBuilderAvailable();

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
        // decodes it back into a workflow_ref.
        runAutomation(data.id, data.source);
        break;
    }
    // Clear the composer once the thread exists. On the onThreadCreated
    // path the panel stays mounted, so a lingering message could be
    // re-submitted and create a duplicate thread.
    setMessage("");
    setMentions(new Map());
    if (onThreadCreated) {
      onThreadCreated(data.id);
    } else {
      navigate(ROUTES.ORG(orgSlug).WORKSPACE(projectId).THREAD(data.id));
    }
  });

  const autoSubmitDone = useRef(false);

  const [autoApprove, setAutoApprove] = useState(
    () => localStorage.getItem("builder_auto_approve") === "true"
  );
  const [message, setMessage] = useState(initialMessage ?? "");

  // Auto-submit when navigated with prefilled question + agent (e.g. from onboarding)
  // biome-ignore lint/correctness/useExhaustiveDependencies: formRef is a stable ref
  useEffect(() => {
    if (autoSubmit && !autoSubmitDone.current && message && agent && !isPending) {
      autoSubmitDone.current = true;
      formRef.current?.requestSubmit();
    }
  }, [autoSubmit, message, agent, isPending]);

  const [cursorPos, setCursorPos] = useState(0);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [mentions, setMentions] = useState<Map<string, string>>(new Map());
  const [mentionDismissed, setMentionDismissed] = useState(false);
  const textareaElRef = useRef<HTMLTextAreaElement | null>(null);
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

  const isBuildMode = mode === "build" && isBuiltin;

  const { data: fileTreeData } = useFileTree(isBuildMode);
  const allFiles = useMemo(() => {
    if (!fileTreeData) return [];
    return flattenFiles(fileTreeData.primary);
  }, [fileTreeData]);

  const activeMention = isBuildMode ? getActiveMention(message, cursorPos) : null;
  const mentionResults = useMemo(() => {
    if (!activeMention) return [];
    const q = activeMention.query.toLowerCase();
    return allFiles
      .filter((f) => f.name.toLowerCase().includes(q) || f.path.toLowerCase().includes(q))
      .slice(0, 8);
  }, [activeMention, allFiles]);
  const showMentionPopup = activeMention !== null && mentionResults.length > 0 && !mentionDismissed;
  const mentionHighlight = useMentionHighlight(message, mentions);

  // biome-ignore lint/correctness/useExhaustiveDependencies: reset on result count change only
  useEffect(() => {
    setSelectedIndex(0);
  }, [mentionResults.length]);

  const textareaRef = useCallback((node: HTMLTextAreaElement | null) => {
    textareaElRef.current = node;
  }, []);

  const insertMention = (file: FileTreeModel) => {
    if (!activeMention) return;
    const before = message.slice(0, activeMention.startIndex);
    const after = message.slice(cursorPos);
    const displayName = getCleanObjectName(file.name);
    const mention = `@${displayName}`;
    const newMessage = `${before}${mention} ${after}`;
    setMessage(newMessage);
    setMentions((prev) => new Map(prev).set(displayName, file.path));
    const newCursorPos = before.length + mention.length + 1;
    setCursorPos(newCursorPos);
    requestAnimationFrame(() => {
      const el = textareaElRef.current;
      if (el) {
        el.focus();
        el.setSelectionRange(newCursorPos, newCursorPos);
      }
    });
  };

  const handleTextareaKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (showMentionPopup) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSelectedIndex((prev) => (prev + 1) % mentionResults.length);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setSelectedIndex((prev) => (prev - 1 + mentionResults.length) % mentionResults.length);
        return;
      }
      if (e.key === "Tab" || e.key === "Enter") {
        e.preventDefault();
        insertMention(mentionResults[selectedIndex]);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setMentionDismissed(true);
        return;
      }
    }
    if (e.key === "Backspace" && isBuildMode) {
      const before = message.slice(0, cursorPos);
      for (const [displayName] of mentions) {
        const withSpace = `@${displayName} `;
        const withoutSpace = `@${displayName}`;
        const removeLen = before.endsWith(withSpace)
          ? withSpace.length
          : before.endsWith(withoutSpace)
            ? withoutSpace.length
            : 0;
        if (removeLen > 0) {
          e.preventDefault();
          const newCursorPos = cursorPos - removeLen;
          setMessage(message.slice(0, newCursorPos) + message.slice(cursorPos));
          setCursorPos(newCursorPos);
          setMentions((prev) => {
            const next = new Map(prev);
            next.delete(displayName);
            return next;
          });
          requestAnimationFrame(() => {
            const el = textareaElRef.current;
            if (el) el.setSelectionRange(newCursorPos, newCursorPos);
          });
          return;
        }
      }
    }
    enterSubmitKeyDown(e);
  };

  const handleTextareaChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setMessage(e.target.value);
    if (isBuildMode) setCursorPos(e.target.selectionStart ?? e.target.value.length);
    setMentionDismissed(false);
  };

  const handleTextareaSelect = (e: React.SyntheticEvent<HTMLTextAreaElement>) => {
    if (isBuildMode) setCursorPos((e.target as HTMLTextAreaElement).selectionStart ?? 0);
  };

  const handleFormSubmit = async (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    if (isPending) return;
    let input = message;
    if (isBuildMode) {
      for (const [displayName, filePath] of mentions) {
        input = input.replaceAll(`@${displayName}`, `<@${filePath}|${displayName}>`);
      }
    }
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
      case "build":
        if (isBuilderAvailable) {
          if (isBuiltin) {
            createThread({
              title: title,
              source: "__builder__",
              source_type: "analytics",
              input
            });
          } else {
            createThread({
              title: title,
              source: builderPath,
              source_type: "task",
              input
            });
          }
        }
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
      case "build":
        return !message || !isBuilderAvailable || isCheckingBuilder;
      case "workflow":
        return !automation;
    }
  };

  const placeholder = (() => {
    if (placeholderOverride && mode === "ask") return placeholderOverride;
    switch (mode) {
      case "ask":
        return "Start your request, and let Oxygen handle everything.";
      case "build":
        return "Enter anything you want to build, and Oxygen will figure out the rest.";
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
      {showMentionPopup && (
        <div className='absolute right-0 bottom-full left-0 z-10 mb-1 max-h-52 overflow-y-auto rounded-md border bg-popover p-1 shadow-md'>
          {mentionResults.map((file, index) => {
            const fileType = detectFileType(file.path);
            const FileIcon = getFileTypeIcon(fileType, file.name);
            return (
              <button
                key={file.path}
                type='button'
                className={cn(
                  "flex w-full cursor-default select-none items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-hidden",
                  index === selectedIndex
                    ? "bg-accent text-accent-foreground"
                    : "text-popover-foreground"
                )}
                onMouseDown={(e) => {
                  e.preventDefault();
                  insertMention(file);
                }}
                onMouseEnter={() => setSelectedIndex(index)}
              >
                {FileIcon && <FileIcon className='size-4 text-muted-foreground' />}
                <span className='flex-1 truncate text-left'>{file.path}</span>
              </button>
            );
          })}
        </div>
      )}
      <HighlightTextarea
        ref={textareaRef}
        disabled={isPending}
        name='question'
        autoFocus
        onKeyDown={handleTextareaKeyDown}
        value={message}
        onChange={handleTextareaChange}
        onSelect={handleTextareaSelect}
        onClick={handleTextareaSelect}
        placeholder={placeholder}
        highlight={isBuildMode ? mentionHighlight : undefined}
        overlayClassName='px-0 py-2 text-sm'
        className='customScrollbar max-h-[200px] resize-none border-none bg-transparent px-0 text-sm shadow-none outline-none placeholder:text-sm hover:border-none focus-visible:border-none focus-visible:shadow-none focus-visible:ring-0 focus-visible:ring-offset-0'
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
                  value='build'
                  disabled={!isBuilderAvailable || isCheckingBuilder}
                  detail={{
                    title: "Build",
                    description:
                      "Build data applications and dashboards by describing what you need in natural language."
                  }}
                >
                  <Hammer className='size-4' />
                  Build
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
          {isBuildMode && (
            <button
              type='button'
              role='switch'
              aria-checked={autoApprove}
              onClick={() => {
                const next = !autoApprove;
                setAutoApprove(next);
                localStorage.setItem("builder_auto_approve", String(next));
              }}
              className={cn(
                "inline-flex h-7 shrink-0 touch-manipulation items-center gap-1 rounded-md px-2 font-medium text-xs transition-colors hover:bg-accent",
                autoApprove ? "text-primary" : "text-muted-foreground hover:text-foreground"
              )}
            >
              <Zap className={cn("h-3 w-3 transition-colors", autoApprove && "fill-primary")} />
              Auto-approve
            </button>
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
