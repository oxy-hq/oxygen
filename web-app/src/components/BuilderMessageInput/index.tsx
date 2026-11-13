import { Zap } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useMentionHighlight } from "@/hooks/useMentionHighlight";
import { cn } from "@/libs/shadcn/utils";
import { getActiveMention, getCleanObjectName } from "@/libs/utils/mention";
import { getFileTypeIcon } from "@/libs/workspaceFiles/utils";
import MessageInputShell from "@/pages/thread/analytics/MessageInputShell";
import type { FileTreeModel } from "@/types/file";

const NO_FILES: FileTreeModel[] = [];

import { detectFileType } from "@/utils/fileTypes";

// --- Component ---

interface BuilderMessageInputProps {
  onSend: (resolvedText: string) => void;
  onStop: () => void;
  disabled: boolean;
  isLoading?: boolean;
  showWarning?: boolean;
  autoApprove?: boolean;
  onAutoApproveChange?: (value: boolean) => void;
}

const BuilderMessageInput = ({
  onSend,
  onStop,
  disabled,
  isLoading = false,
  showWarning = false,
  autoApprove = false,
  onAutoApproveChange
}: BuilderMessageInputProps) => {
  const [message, setMessage] = useState("");
  const [cursorPos, setCursorPos] = useState(0);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [mentions, setMentions] = useState<Map<string, string>>(new Map());
  const [mentionDismissed, setMentionDismissed] = useState(false);
  const textareaElRef = useRef<HTMLTextAreaElement | null>(null);

  // File mentions read the working copy's file tree, which was removed with
  // the IDE: nothing to suggest.
  const allFiles: FileTreeModel[] = NO_FILES;

  const mentionHighlight = useMentionHighlight(message, mentions);

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
  }, [activeMention]);

  const showMentionPopup = activeMention !== null && mentionResults.length > 0 && !mentionDismissed;

  // biome-ignore lint/correctness/useExhaustiveDependencies: reset on result count change only
  useEffect(() => {
    setSelectedIndex(0);
  }, [mentionResults.length]);

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

  // Convert @label → [label](path) so the backend stores the label explicitly.
  const resolveInput = (text: string) => {
    let resolved = text;
    for (const [displayName, filePath] of mentions) {
      resolved = resolved.replaceAll(`@${displayName}`, `<@${filePath}|${displayName}>`);
    }
    return resolved;
  };

  const handleSend = () => {
    if (!message.trim() || disabled) return;
    onSend(resolveInput(message));
    setMessage("");
    setCursorPos(0);
    setMentions(new Map());
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
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
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
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

  return (
    <MessageInputShell
      value={message}
      onChange={handleChange}
      onKeyDown={handleKeyDown}
      onSend={handleSend}
      onStop={onStop}
      disabled={disabled}
      isLoading={isLoading}
      showWarning={showWarning}
      placeholder='Ask a follow-up question...'
      textareaRef={textareaElRef}
      onSelect={handleSelect}
      onClick={handleSelect}
      highlight={mentionHighlight}
      aboveInput={
        showMentionPopup ? (
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
        ) : undefined
      }
      extraActions={
        onAutoApproveChange ? (
          <button
            type='button'
            role='switch'
            aria-checked={autoApprove}
            onClick={() => onAutoApproveChange(!autoApprove)}
            className={cn(
              "inline-flex h-7 shrink-0 touch-manipulation items-center gap-1 rounded-md px-2 font-medium text-xs transition-colors hover:bg-accent",
              autoApprove ? "text-primary" : "text-muted-foreground hover:text-foreground"
            )}
          >
            <Zap className={cn("h-3 w-3 transition-colors", autoApprove && "fill-primary")} />
            Auto-approve
          </button>
        ) : undefined
      }
    />
  );
};

export default BuilderMessageInput;
