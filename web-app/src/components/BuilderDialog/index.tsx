import { Hammer, Loader2, Zap } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import useFileTree from "@/hooks/api/files/useFileTree";
import useModelingProjects from "@/hooks/api/modeling/useModelingProjects";
import useThread from "@/hooks/api/threads/useThread";
import useThreadMutation from "@/hooks/api/threads/useThreadMutation";
import useBuilderAvailable from "@/hooks/api/useBuilderAvailable";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { useEnterSubmit } from "@/hooks/useEnterSubmit";
import { useMentionHighlight } from "@/hooks/useMentionHighlight";
import { cn } from "@/libs/shadcn/utils";
import { flattenFiles, getActiveMention, getCleanObjectName } from "@/libs/utils/mention";
import ROUTES from "@/libs/utils/routes";
import { getShortTitle } from "@/libs/utils/string";
import { getFileTypeIcon } from "@/pages/ide/Files/FilesSidebar/utils";
import { AnalyticsService } from "@/services/api";
import useBuilderDialog from "@/stores/useBuilderDialog";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { FileTreeModel } from "@/types/file";
import { decodeFilePath, detectFileType } from "@/utils/fileTypes";
import { HighlightTextarea } from "../ui/HighlightTextarea";
import { Dialog, DialogContent } from "../ui/shadcn/dialog";

// --- Helpers ---

function extractFileContext(pathname: string) {
  // `/automations` is the canonical route; `/workflows` is kept as a back-compat alias.
  const match = pathname.match(/(?:\/ide\/files|\/apps|\/automations|\/workflows)\/([^/]+)/);
  if (!match) return null;
  const pathb64 = match[1];
  const filePath = decodeFilePath(pathb64);
  if (!filePath) return null;
  const fileName = filePath.split("/").pop() || filePath;
  const fileType = detectFileType(filePath);
  const displayName = getCleanObjectName(fileName);
  const Icon = getFileTypeIcon(fileType, fileName);
  return { pathb64, filePath, fileName, displayName, fileType, Icon };
}

// --- Component ---

export function BuilderDialog() {
  const { isOpen, setIsOpen, modelingSelection } = useBuilderDialog();
  const navigate = useNavigate();
  const location = useLocation();
  const { project } = useCurrentProjectBranch();
  const projectId = project.id;
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";

  const [autoApprove, setAutoApprove] = useState(
    () => localStorage.getItem("builder_auto_approve") === "true"
  );
  const [message, setMessage] = useState("");
  const [cursorPos, setCursorPos] = useState(0);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [mentions, setMentions] = useState<Map<string, string>>(new Map());
  const [mentionDismissed, setMentionDismissed] = useState(false);
  const { formRef, onKeyDown: enterSubmitKeyDown } = useEnterSubmit();
  const textareaElRef = useRef<HTMLTextAreaElement | null>(null);
  const insertMentionRafRef = useRef<number | null>(null);
  const placeCursorAtEndRef = useRef(false);

  useEffect(() => {
    return () => {
      if (insertMentionRafRef.current !== null) cancelAnimationFrame(insertMentionRafRef.current);
    };
  }, []);

  const {
    isAvailable,
    isLoading: isCheckingBuilder,
    isBuiltin,
    builderModel,
    builderPath
  } = useBuilderAvailable();

  useEffect(() => {
    if (isOpen && !isCheckingBuilder && !isBuiltin) {
      setIsOpen(false);
    }
  }, [isOpen, isCheckingBuilder, isBuiltin, setIsOpen]);

  const { data: fileTreeData } = useFileTree(isOpen && isBuiltin);
  const { data: modelingProjects } = useModelingProjects();

  const allFiles = useMemo(() => {
    if (!fileTreeData) return [];
    return flattenFiles(fileTreeData.primary);
  }, [fileTreeData]);

  const fileContext = useMemo(() => extractFileContext(location.pathname), [location.pathname]);
  const fileDisplayName = fileContext?.displayName;
  const isOnModelingPage = location.pathname.includes("/ide/modeling");

  // Extract thread ID if we're on a thread page
  const threadId = useMemo(() => {
    const match = location.pathname.match(/\/threads\/([^/]+)/);
    return match ? match[1] : null;
  }, [location.pathname]);

  const { data: threadData } = useThread(threadId ?? "", isOpen && !!threadId && !fileContext);

  // Mention autocomplete state
  const activeMention = getActiveMention(message, cursorPos);
  const mentionResults = useMemo(() => {
    if (!activeMention) return [];
    const q = activeMention.query.toLowerCase();
    return allFiles
      .filter((f) => {
        const name = f.name.toLowerCase();
        const path = f.path.toLowerCase();
        return name.includes(q) || path.includes(q);
      })
      .slice(0, 8);
  }, [activeMention, allFiles]);

  const showMentionPopup = activeMention !== null && mentionResults.length > 0 && !mentionDismissed;

  // Reset selected index when results change
  // biome-ignore lint/correctness/useExhaustiveDependencies: reset on result count change only
  useEffect(() => {
    setSelectedIndex(0);
  }, [mentionResults.length]);

  const textareaRef = useCallback((node: HTMLTextAreaElement | null) => {
    textareaElRef.current = node;
  }, []);

  // Place cursor at end after message updates (handles pre-filled @mention).
  // setTimeout(0) runs after all RAFs including Radix's FocusScope, which
  // otherwise resets the cursor when it re-focuses the textarea post-animation.
  // biome-ignore lint/correctness/useExhaustiveDependencies: message is used as a trigger only — the effect reads refs, not message itself
  useEffect(() => {
    if (!placeCursorAtEndRef.current) return;
    placeCursorAtEndRef.current = false;
    const node = textareaElRef.current;
    if (!node) return;
    const id = setTimeout(() => {
      node.focus();
      node.setSelectionRange(node.value.length, node.value.length);
    }, 0);
    return () => clearTimeout(id);
  }, [message]);

  // Pre-fill @mention when dialog opens with file context or thread agent
  useEffect(() => {
    if (isOpen && isOnModelingPage) {
      if (modelingSelection) {
        const project = modelingProjects?.find(
          (p) => (p.folder_name || p.name) === modelingSelection.projectName
        );
        if (project) {
          const folderName = project.folder_name || project.name;
          const projectRelDir = `modeling/${folderName}`;
          const mentionMap = new Map<string, string>();
          const parts: string[] = [];
          if (modelingSelection.node) {
            const modelRoot = project.model_paths[0] ?? "models";
            const nodePath = `${projectRelDir}/${modelRoot}/${modelingSelection.node.path}`;
            const nodeDisplayName = modelingSelection.node.name.replace(/\.sql$/, "");
            mentionMap.set(nodeDisplayName, nodePath);
            parts.push(`@${nodeDisplayName}`);
          } else {
            mentionMap.set(project.name, `${projectRelDir}/dbt_project.yml`);
            parts.push(`@${project.name}`);
          }
          placeCursorAtEndRef.current = true;
          setMessage(`${parts.join(" ")} `);
          setMentions(mentionMap);
        }
      }
    } else if (isOpen && fileDisplayName && fileContext) {
      const dbtProject = modelingProjects?.find(
        (p) =>
          fileContext.filePath.startsWith(`${p.project_dir}/`) ||
          fileContext.filePath.startsWith(`modeling/${p.folder_name || p.name}/`)
      );
      const mentionMap = new Map([[fileDisplayName, fileContext.filePath]]);
      if (dbtProject) {
        const folderName = dbtProject.folder_name || dbtProject.name;
        mentionMap.set(dbtProject.name, `modeling/${folderName}/dbt_project.yml`);
      }
      const msg = dbtProject ? `@${fileDisplayName} @${dbtProject.name} ` : `@${fileDisplayName} `;
      placeCursorAtEndRef.current = true;
      setMessage(msg);
      setMentions(mentionMap);
    } else if (isOpen && threadData?.source && threadData.source !== "__builder__") {
      const agentFile = allFiles.find((f) => f.path === threadData.source);
      const displayName = agentFile
        ? getCleanObjectName(agentFile.name)
        : getCleanObjectName(threadData.source.split("/").pop() ?? threadData.source);
      const filePath = agentFile?.path ?? threadData.source;
      placeCursorAtEndRef.current = true;
      setMessage(`@${displayName} `);
      setMentions(new Map([[displayName, filePath]]));
    }
    if (!isOpen) {
      setMessage("");
      setCursorPos(0);
      setMentions(new Map());
    }
  }, [
    isOpen,
    isOnModelingPage,
    fileDisplayName,
    fileContext,
    threadData,
    allFiles,
    modelingProjects,
    modelingSelection
  ]);

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
    if (insertMentionRafRef.current !== null) cancelAnimationFrame(insertMentionRafRef.current);
    insertMentionRafRef.current = requestAnimationFrame(() => {
      insertMentionRafRef.current = null;
      const el = textareaElRef.current;
      if (el) {
        el.focus();
        el.setSelectionRange(newCursorPos, newCursorPos);
      }
    });
  };

  const { mutate: createThread, isPending } = useThreadMutation((data) => {
    switch (data.source_type) {
      case "analytics":
        AnalyticsService.createRun(projectId, {
          agent_id: data.source,
          question: data.input,
          thread_id: data.id,
          ...(data.source === "__builder__" && {
            domain: "builder",
            model: builderModel
          })
        });
        break;
    }
    setIsOpen(false);
    setMessage("");
    const threadUri = ROUTES.ORG(orgSlug).WORKSPACE(projectId).THREAD(data.id);
    navigate(threadUri);
  });

  const handleSubmit = (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    if (!message.trim() || !isAvailable || isCheckingBuilder) return;

    let input = message;
    for (const [displayName, filePath] of mentions) {
      input = input.replaceAll(`@${displayName}`, `<@${filePath}|${displayName}>`);
    }
    const title = getShortTitle(message);

    if (isBuiltin) {
      createThread({
        title,
        source: "__builder__",
        source_type: "analytics",
        input
      });
    } else {
      createThread({
        title,
        source: builderPath,
        source_type: "task",
        input
      });
    }
  };

  const handleOpenChange = (open: boolean) => {
    setIsOpen(open);
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.metaKey && e.key === "i") {
      e.preventDefault();
      setIsOpen(false);
      return;
    }
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
    if (e.key === "Backspace") {
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

  const handleChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    setMessage(e.target.value);
    setCursorPos(e.target.selectionStart ?? e.target.value.length);
    setMentionDismissed(false);
  };

  const handleSelect = (e: React.SyntheticEvent<HTMLTextAreaElement>) => {
    const target = e.target as HTMLTextAreaElement;
    setCursorPos(target.selectionStart ?? 0);
  };

  const mentionHighlight = useMentionHighlight(message, mentions);

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent
        showCloseButton={false}
        data-testid='builder-dialog-root'
        className='top-[30%] max-w-150 translate-y-0 gap-0 overflow-hidden border-white/10 bg-popover p-0 shadow-[0_8px_60px_rgba(0,0,0,0.5),0_0_0_1px_rgba(255,255,255,0.08),0_0_30px_-4px_color-mix(in_srgb,var(--primary)_15%,transparent)] backdrop-blur-2xl dark:bg-popover/50'
      >
        {!isAvailable && !isCheckingBuilder ? (
          <div className='p-6 text-center text-muted-foreground text-sm'>
            Builder is not available for this project.
          </div>
        ) : (
          <form ref={formRef} onSubmit={handleSubmit} className='flex flex-col'>
            {/* Top bar */}
            <div className='flex items-center gap-2 border-b px-3 py-2'>
              <span className='inline-flex items-center gap-1 rounded-md bg-vis-orange/15 px-2 py-0.5 font-medium text-vis-orange text-xs'>
                <Hammer className='size-3' />
                Build
              </span>
              {isBuiltin && (
                <span className='text-muted-foreground text-xs'>
                  <kbd className='rounded border bg-muted px-1 py-0.5 font-mono text-[10px]'>@</kbd>{" "}
                  to mention
                </span>
              )}
              <span className='ml-auto text-muted-foreground text-xs'>
                <kbd className='rounded border bg-muted px-1 py-0.5 font-mono text-[10px]'>
                  Enter
                </kbd>{" "}
                to build
              </span>
            </div>

            {/* Textarea */}
            <HighlightTextarea
              ref={textareaRef}
              autoFocus
              name='builder-input'
              data-testid='builder-input-textarea'
              disabled={isPending}
              onKeyDown={handleKeyDown}
              value={message}
              onChange={handleChange}
              onSelect={handleSelect}
              onClick={handleSelect}
              placeholder='Describe what you want to build...'
              className='customScrollbar max-h-50 min-h-20 resize-none border-none bg-transparent py-3 text-sm shadow-none outline-none focus-visible:ring-0 focus-visible:ring-offset-0'
              highlight={mentionHighlight}
              overlayClassName='px-3 py-3 text-sm'
            />

            {/* Mention autocomplete dropdown */}
            {isBuiltin && showMentionPopup && (
              <div className='max-h-52 overflow-y-auto border-t bg-popover p-1'>
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

            {/* Footer */}
            <div className='flex items-center justify-between border-t px-3 py-2'>
              <div className='flex items-center gap-2'>
                {isPending && <Loader2 className='size-3.5 animate-spin text-muted-foreground' />}
                <span className='text-muted-foreground text-xs'>
                  Press{" "}
                  <kbd className='rounded border bg-muted px-1 py-0.5 font-mono text-[10px]'>
                    Esc
                  </kbd>{" "}
                  to cancel.
                </span>
              </div>
              {isBuiltin && (
                <button
                  type='button'
                  data-testid='builder-auto-approve-toggle'
                  data-state={autoApprove ? "on" : "off"}
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
            </div>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}
