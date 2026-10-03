import { ArrowRight, ArrowUp, ChevronLeft, ChevronRight, MessageSquare } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { HighlightTextarea } from "@/components/ui/HighlightTextarea";
import { Button } from "@/components/ui/shadcn/button";
import useFileTree from "@/hooks/api/files/useFileTree";
import { useMentionHighlight } from "@/hooks/useMentionHighlight";
import { cn } from "@/libs/shadcn/utils";
import { flattenFiles, getActiveMention, getCleanObjectName } from "@/libs/utils/mention";
import { getFileTypeIcon } from "@/pages/ide/Files/FilesSidebar/utils";
import type { HumanInputQuestion } from "@/services/api/analytics";
import type { FileTreeModel } from "@/types/file";
import { detectFileType } from "@/utils/fileTypes";
import FileChangeDiff, { parseFileChange } from "./FileChangeDiff";

/** Returns a human-readable string from a prompt that may be raw JSON. */
function parsePromptText(prompt: string): string {
  try {
    const parsed = JSON.parse(prompt);
    if (parsed && typeof parsed.description === "string") return parsed.description;
  } catch {
    // not JSON
  }
  return prompt;
}

/** Returns true when the prompt is a structured backend JSON (has a `type` field).
 *  These prompts use suggestion buttons only — no free-text input. */
function isStructuredPrompt(prompt: string): boolean {
  try {
    const parsed = JSON.parse(prompt);
    return parsed !== null && typeof parsed === "object" && typeof parsed.type === "string";
  } catch {
    return false;
  }
}

interface SuspensionPromptProps {
  questions: HumanInputQuestion[];
  onAnswer: (text: string) => void;
  isAnswering: boolean;
}

const SuspensionPrompt = ({ questions, onAnswer, isAnswering }: SuspensionPromptProps) => {
  const [answers, setAnswersState] = useState<string[]>(() => questions.map(() => ""));
  const [resolvedAnswers, setResolvedAnswers] = useState<string[]>(() => questions.map(() => ""));
  const [mentionMaps, setMentionMaps] = useState<Map<string, string>[]>(() =>
    questions.map(() => new Map())
  );
  const [activeIndex, setActiveIndex] = useState(0);
  const [cursorPositions, setCursorPositions] = useState<number[]>(() => questions.map(() => 0));
  const [selectedMentionIndex, setSelectedMentionIndex] = useState(0);
  const [mentionDismissed, setMentionDismissed] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  const safeIndex = Math.min(activeIndex, Math.max(0, questions.length - 1));
  const activeQuestion = questions[safeIndex];

  const cursorPos = cursorPositions[safeIndex] ?? 0;
  const setCursorPos = (val: number) => {
    const idx = safeIndex;
    setCursorPositions((prev) => {
      const next = [...prev];
      next[idx] = val;
      return next;
    });
  };

  const { data: fileTreeData } = useFileTree();
  const allFiles = useMemo(() => {
    if (!fileTreeData) return [];
    return flattenFiles(fileTreeData.primary);
  }, [fileTreeData]);

  const activeMention = getActiveMention(answers[safeIndex], cursorPos);
  const mentionResults = useMemo(() => {
    if (!activeMention) return [];
    const q = activeMention.query.toLowerCase();
    return allFiles
      .filter((f) => f.name.toLowerCase().includes(q) || f.path.toLowerCase().includes(q))
      .slice(0, 8);
  }, [activeMention, allFiles]);

  const showMentionPopup = activeMention !== null && mentionResults.length > 0 && !mentionDismissed;
  const mentionHighlight = useMentionHighlight(answers[safeIndex], mentionMaps[safeIndex]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: reset on result count change only
  useEffect(() => {
    setSelectedMentionIndex(0);
  }, [mentionResults.length]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: intentionally reset dismissed state when question changes
  useEffect(() => {
    setMentionDismissed(false);
  }, [activeIndex]);

  const resolveText = (text: string, mentionsMap: Map<string, string>) => {
    let resolved = text;
    for (const [displayName, filePath] of mentionsMap) {
      resolved = resolved.replaceAll(`@${displayName}`, `<@${filePath}|${displayName}>`);
    }
    return resolved;
  };

  const setAnswer = (index: number, raw: string, resolved?: string) => {
    setAnswersState((prev) => {
      const next = [...prev];
      next[index] = raw;
      return next;
    });
    setResolvedAnswers((prev) => {
      const next = [...prev];
      next[index] = resolved ?? raw;
      return next;
    });
  };

  const insertMention = (file: FileTreeModel) => {
    if (!activeMention) return;
    const current = answers[safeIndex];
    const before = current.slice(0, activeMention.startIndex);
    const after = current.slice(cursorPos);
    const displayName = getCleanObjectName(file.name);
    const mention = `@${displayName}`;
    const newValue = `${before}${mention} ${after}`;
    const newMentionMap = new Map(mentionMaps[safeIndex]).set(displayName, file.path);
    setMentionMaps((prev) => {
      const next = [...prev];
      next[safeIndex] = newMentionMap;
      return next;
    });
    setAnswer(safeIndex, newValue, resolveText(newValue, newMentionMap));
    const newCursorPos = before.length + mention.length + 1;
    setCursorPos(newCursorPos);
    requestAnimationFrame(() => {
      const el = textareaRef.current;
      if (el) {
        el.focus();
        el.setSelectionRange(newCursorPos, newCursorPos);
      }
    });
  };

  const submitAll = () => {
    const combined =
      questions.length === 1
        ? resolvedAnswers[0].trim()
        : questions.map((q, i) => `Q: ${q.prompt}\nA: ${resolvedAnswers[i].trim()}`).join("\n\n");
    if (!combined) return;
    onAnswer(combined);
    setAnswersState(questions.map(() => ""));
    setResolvedAnswers(questions.map(() => ""));
    setMentionMaps(questions.map(() => new Map()));
  };

  const allFilled = answers.every((a) => a.trim().length > 0);

  const nextUnansweredIndex = (() => {
    for (let i = safeIndex + 1; i < questions.length; i++) {
      if (!answers[i].trim()) return i;
    }
    for (let i = 0; i < safeIndex; i++) {
      if (!answers[i].trim()) return i;
    }
    return null;
  })();

  const handleNextOrSubmit = () => {
    if (allFilled) {
      submitAll();
    } else if (nextUnansweredIndex !== null) {
      setActiveIndex(nextUnansweredIndex);
    }
  };

  const isNextOrSubmitDisabled = isAnswering || (!answers[safeIndex].trim() && !allFilled);

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (showMentionPopup) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSelectedMentionIndex((prev) => (prev + 1) % mentionResults.length);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setSelectedMentionIndex(
          (prev) => (prev - 1 + mentionResults.length) % mentionResults.length
        );
        return;
      }
      if (e.key === "Tab" || e.key === "Enter") {
        e.preventDefault();
        insertMention(mentionResults[selectedMentionIndex]);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setMentionDismissed(true);
        return;
      }
    }
    if (e.key === "Backspace") {
      const before = answers[safeIndex].slice(0, cursorPos);
      const currentMentionMap = mentionMaps[safeIndex];
      for (const [displayName] of currentMentionMap) {
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
          const newValue =
            answers[safeIndex].slice(0, newCursorPos) + answers[safeIndex].slice(cursorPos);
          const newMentionMap = new Map(currentMentionMap);
          newMentionMap.delete(displayName);
          setMentionMaps((prev) => {
            const next = [...prev];
            next[safeIndex] = newMentionMap;
            return next;
          });
          setAnswer(safeIndex, newValue, resolveText(newValue, newMentionMap));
          setCursorPos(newCursorPos);
          requestAnimationFrame(() => {
            const el = textareaRef.current;
            if (el) el.setSelectionRange(newCursorPos, newCursorPos);
          });
          return;
        }
      }
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      if (questions.length === 1) {
        submitAll();
      } else {
        handleNextOrSubmit();
      }
    }
  };

  // Single file_change question → render the diff UI instead of the card.
  if (questions.length === 1) {
    const payload = parseFileChange(questions[0].prompt);
    if (payload) {
      return (
        <div className='rounded-lg border border-border bg-muted/30 p-4'>
          <FileChangeDiff
            payload={payload}
            suggestions={questions[0].suggestions}
            onAnswer={onAnswer}
            isAnswering={isAnswering}
          />
        </div>
      );
    }
  }

  return (
    <div className='rounded-lg border border-border bg-muted/30'>
      {/* Card navigation header */}
      <div className='flex items-center gap-2 border-border border-b px-3 py-1.5'>
        <MessageSquare className='h-3 w-3 shrink-0 text-muted-foreground' />
        <span className='flex-1 text-muted-foreground text-sm'>
          {questions.length === 1 ? "Question" : `${questions.length} questions`}
        </span>

        {questions.length > 1 && (
          <div className='flex items-center gap-1'>
            <button
              type='button'
              onClick={() => setActiveIndex((i) => Math.max(0, i - 1))}
              disabled={safeIndex === 0}
              className='rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground disabled:opacity-30'
              aria-label='Previous question'
            >
              <ChevronLeft className='h-3.5 w-3.5' />
            </button>
            <span className='min-w-[3rem] text-center font-mono text-muted-foreground text-xs'>
              {safeIndex + 1} / {questions.length}
            </span>
            <button
              type='button'
              onClick={() => setActiveIndex((i) => Math.min(questions.length - 1, i + 1))}
              disabled={safeIndex >= questions.length - 1}
              className='rounded p-0.5 text-muted-foreground transition-colors hover:text-foreground disabled:opacity-30'
              aria-label='Next question'
            >
              <ChevronRight className='h-3.5 w-3.5' />
            </button>
          </div>
        )}
      </div>

      {/* Active question content */}
      <div className='space-y-3 p-3'>
        <p className='font-medium text-sm'>{parsePromptText(activeQuestion.prompt)}</p>

        {activeQuestion.suggestions.length > 0 && (
          <div className='flex flex-wrap gap-2'>
            {activeQuestion.suggestions.map((s) => (
              <button
                key={s}
                type='button'
                onClick={() => {
                  if (questions.length === 1) {
                    onAnswer(s);
                  } else {
                    setAnswer(safeIndex, s);
                  }
                }}
                disabled={isAnswering}
                className='rounded-full border border-border px-3 py-1 text-xs transition-colors hover:bg-accent disabled:opacity-50'
              >
                {s}
              </button>
            ))}
          </div>
        )}

        {!isStructuredPrompt(activeQuestion.prompt) && (
          <div className='relative'>
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
                        index === selectedMentionIndex
                          ? "bg-accent text-accent-foreground"
                          : "text-popover-foreground"
                      )}
                      onMouseDown={(e) => {
                        e.preventDefault();
                        insertMention(file);
                      }}
                      onMouseEnter={() => setSelectedMentionIndex(index)}
                    >
                      {FileIcon && <FileIcon className='size-4 text-muted-foreground' />}
                      <span className='flex-1 truncate text-left'>{file.path}</span>
                    </button>
                  );
                })}
              </div>
            )}
            <div className='overflow-hidden rounded-md border border-border bg-secondary transition-shadow focus-within:border-ring focus-within:ring-[3px] focus-within:ring-ring/50'>
              <HighlightTextarea
                ref={textareaRef}
                value={answers[safeIndex]}
                highlight={mentionHighlight}
                className='min-h-[60px] resize-none rounded-none border-0 bg-transparent shadow-none focus-visible:ring-0'
                onChange={(e: React.ChangeEvent<HTMLTextAreaElement>) => {
                  const raw = e.target.value;
                  setCursorPos(e.target.selectionStart ?? raw.length);
                  setMentionDismissed(false);
                  setAnswer(safeIndex, raw, resolveText(raw, mentionMaps[safeIndex]));
                }}
                onKeyDown={handleKeyDown}
                onSelect={(e: React.SyntheticEvent<HTMLTextAreaElement>) => {
                  setCursorPos((e.target as HTMLTextAreaElement).selectionStart ?? 0);
                }}
                onClick={(e: React.MouseEvent<HTMLTextAreaElement>) => {
                  setCursorPos((e.target as HTMLTextAreaElement).selectionStart ?? 0);
                }}
                placeholder='Type your answer…'
                disabled={isAnswering}
              />
              <div className='flex items-center justify-end border-border border-t bg-secondary px-2 py-1.5'>
                <Button
                  size='icon'
                  className='size-7'
                  onClick={questions.length === 1 ? submitAll : handleNextOrSubmit}
                  disabled={
                    questions.length === 1 ? isAnswering || !allFilled : isNextOrSubmitDisabled
                  }
                  aria-label={
                    questions.length === 1
                      ? "Send answer"
                      : allFilled
                        ? "Send answers"
                        : "Next unanswered question"
                  }
                >
                  {questions.length > 1 && !allFilled ? (
                    <ArrowRight className='size-4' />
                  ) : (
                    <ArrowUp className='size-4' />
                  )}
                </Button>
              </div>
            </div>
          </div>
        )}
      </div>

      {/* Dot navigation */}
      {questions.length > 1 && (
        <div className='flex justify-center gap-1.5 px-3 pb-3'>
          {questions.map((_, i) => (
            <button
              // biome-ignore lint/suspicious/noArrayIndexKey: index is stable for fixed-count dots
              key={i}
              type='button'
              onClick={() => setActiveIndex(i)}
              aria-label={`Question ${i + 1}`}
              className={cn(
                "h-1.5 rounded-full transition-all",
                i === safeIndex
                  ? "w-4 bg-primary"
                  : answers[i].trim()
                    ? "w-1.5 bg-primary/40 hover:bg-primary/60"
                    : "w-1.5 bg-muted-foreground/30 hover:bg-muted-foreground/60"
              )}
            />
          ))}
        </div>
      )}
    </div>
  );
};

export default SuspensionPrompt;
