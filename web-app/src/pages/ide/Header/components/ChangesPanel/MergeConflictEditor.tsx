import { useQuery } from "@tanstack/react-query";
import { AlertTriangle, CheckCircle2, ChevronsDown, RotateCcw } from "lucide-react";
import type { editor } from "monaco-editor";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ErrorBoundary } from "react-error-boundary";
import { toast } from "sonner";
import { getLanguageFromFileName } from "@/components/FileEditor/constants";
import { BaseMonacoEditor } from "@/components/MonacoEditor";
import ErrorAlert from "@/components/ui/ErrorAlert";
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup
} from "@/components/ui/shadcn/resizable";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useAuth } from "@/contexts/AuthContext";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { encodeBase64 } from "@/libs/encoding";
import { FileService } from "@/services/api";
import { WorkspaceService as ProjectService } from "@/services/api/workspaces";
import type { FileStatus } from "@/types/file";

type ConflictAction = "mine" | "theirs" | "both" | "ignored";

interface ConflictBlock {
  index: number;
  startLine: number; // <<<<<<< (1-based)
  baseLine?: number; // ||||||| diff3
  equalLine: number; // =======
  endLine: number; // >>>>>>>
  mineLines: string[];
  baseLines: string[];
  theirsLines: string[];
}

type DecIds = string[];
interface ZoneRec {
  id: string;
  dom: HTMLElement;
}

function parseConflicts(content: string): ConflictBlock[] {
  const lines = content.split("\n");
  const blocks: ConflictBlock[] = [];
  let state: "normal" | "mine" | "base" | "theirs" = "normal";
  let startLine = 0,
    baseLine: number | undefined,
    equalLine = 0;
  let mineLines: string[] = [],
    baseLines: string[] = [],
    theirsLines: string[] = [];

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const n = i + 1;
    if (line.startsWith("<<<<<<<") && state === "normal") {
      state = "mine";
      startLine = n;
      mineLines = [];
      baseLines = [];
      theirsLines = [];
      baseLine = undefined;
    } else if (line.startsWith("|||||||") && state === "mine") {
      state = "base";
      baseLine = n;
    } else if (line.startsWith("=======") && (state === "mine" || state === "base")) {
      state = "theirs";
      equalLine = n;
    } else if (line.startsWith(">>>>>>>") && state === "theirs") {
      blocks.push({
        index: blocks.length,
        startLine,
        baseLine,
        equalLine,
        endLine: n,
        mineLines: [...mineLines],
        baseLines: [...baseLines],
        theirsLines: [...theirsLines]
      });
      state = "normal";
    } else if (state === "mine") {
      mineLines.push(line);
    } else if (state === "base") {
      baseLines.push(line);
    } else if (state === "theirs") {
      theirsLines.push(line);
    }
  }
  return blocks;
}

function hasMarkers(content: string): boolean {
  return content.includes("<<<<<<<") || content.includes("|||||||");
}

function resolveSideWithRanges(
  content: string,
  side: "mine" | "theirs"
): { text: string; ranges: { startLine: number; endLine: number }[] } {
  const lines = content.split("\n");
  const out: string[] = [];
  const ranges: { startLine: number; endLine: number }[] = [];
  let state: "normal" | "mine" | "base" | "theirs" = "normal";
  let rangeStart = 0;

  for (const line of lines) {
    if (line.startsWith("<<<<<<<")) {
      state = "mine";
      rangeStart = out.length + 1;
    } else if (line.startsWith("|||||||") && state !== "normal") {
      state = "base";
    } else if (line.startsWith("=======") && state !== "normal") {
      state = "theirs";
    } else if (line.startsWith(">>>>>>>") && state !== "normal") {
      if (out.length >= rangeStart) ranges.push({ startLine: rangeStart, endLine: out.length });
      state = "normal";
    } else if (state === "normal" || state === side) {
      out.push(line);
    }
  }
  return { text: out.join("\n"), ranges };
}

function resolveBlock(content: string, blockIndex: number, action: ConflictAction): string {
  const blocks = parseConflicts(content);
  const block = blocks[blockIndex];
  if (!block) return content;

  let chosen: string[];
  switch (action) {
    case "mine":
      chosen = block.mineLines;
      break;
    case "theirs":
      chosen = block.theirsLines;
      break;
    case "both":
      chosen = [...block.mineLines, ...block.theirsLines];
      break;
    case "ignored":
      chosen = [];
      break;
    default:
      return content;
  }

  const lines = content.split("\n");
  return [...lines.slice(0, block.startLine - 1), ...chosen, ...lines.slice(block.endLine)].join(
    "\n"
  );
}

function resolveAll(content: string, action: ConflictAction): string {
  let c = content;
  for (let i = 0; hasMarkers(c) && i < 500; i++) {
    const blocks = parseConflicts(c);
    if (!blocks.length) break;
    c = resolveBlock(c, 0, action);
  }
  return c;
}

const STYLE_ID = "oxy-merge-editor-styles";

function injectStyles() {
  if (document.getElementById(STYLE_ID)) return;
  const s = document.createElement("style");
  s.id = STYLE_ID;
  s.textContent = `
    .cmx-mine   { background: color-mix(in srgb, var(--conflict-mine) 9%, transparent) !important; border-left: 2px solid color-mix(in srgb, var(--conflict-mine) 35%, transparent) !important; }
    .cmx-theirs { background: color-mix(in srgb, var(--conflict-theirs) 9%, transparent) !important; border-left: 2px solid color-mix(in srgb, var(--conflict-theirs) 35%, transparent) !important; }
    .cmx-base   { background: color-mix(in srgb, var(--muted-foreground) 5%, transparent) !important; }
    .cmx-marker { background: color-mix(in srgb, var(--warning) 14%, transparent) !important; }
    .cmx-hl-mine   { background: color-mix(in srgb, var(--conflict-mine) 11%, transparent) !important; border-left: 2px solid color-mix(in srgb, var(--conflict-mine) 50%, transparent) !important; }
    .cmx-hl-theirs { background: color-mix(in srgb, var(--conflict-theirs) 11%, transparent) !important; border-left: 2px solid color-mix(in srgb, var(--conflict-theirs) 50%, transparent) !important; }
    /* zone bars live inside Monaco's overflow-visible layer — need z-index */
    .cmx-zone { z-index: 1; }
  `;
  document.head.appendChild(s);
}

function applyResultDecs(
  ed: editor.IStandaloneCodeEditor,
  conflicts: ConflictBlock[],
  prev: DecIds
): DecIds {
  const decs: editor.IModelDeltaDecoration[] = [];
  const d = (s: number, e: number, cls: string) =>
    decs.push({
      range: {
        startLineNumber: s,
        startColumn: 1,
        endLineNumber: e,
        endColumn: 9999
      },
      options: { isWholeLine: true, className: cls }
    });

  for (const c of conflicts) {
    d(c.startLine, c.startLine, "cmx-marker");
    d(c.equalLine, c.equalLine, "cmx-marker");
    d(c.endLine, c.endLine, "cmx-marker");
    if (c.baseLine) d(c.baseLine, c.baseLine, "cmx-marker");

    const mineEnd = (c.baseLine ?? c.equalLine) - 1;
    if (mineEnd >= c.startLine + 1) d(c.startLine + 1, mineEnd, "cmx-mine");

    if (c.baseLine) {
      const baseEnd = c.equalLine - 1;
      if (baseEnd >= c.baseLine + 1) d(c.baseLine + 1, baseEnd, "cmx-base");
    }

    const theirsEnd = c.endLine - 1;
    if (theirsEnd >= c.equalLine + 1) d(c.equalLine + 1, theirsEnd, "cmx-theirs");
  }

  return ed.deltaDecorations(prev, decs);
}

function applyHighlightDecs(
  ed: editor.IStandaloneCodeEditor,
  ranges: { startLine: number; endLine: number }[],
  cls: string,
  prev: DecIds
): DecIds {
  return ed.deltaDecorations(
    prev,
    ranges.map((r) => ({
      range: {
        startLineNumber: r.startLine,
        startColumn: 1,
        endLineNumber: r.endLine,
        endColumn: 9999
      },
      options: { isWholeLine: true, className: cls }
    }))
  );
}

function makeSpacerDom(): HTMLElement {
  const div = document.createElement("div");
  div.style.height = "22px";
  return div;
}

function clearZones(ed: editor.IStandaloneCodeEditor, recs: ZoneRec[]) {
  if (!recs.length) return;
  ed.changeViewZones((a) => {
    for (const z of recs) {
      a.removeZone(z.id);
    }
  });
}

// Zones only reserve vertical space — actual buttons are React overlays.
function injectZones(ed: editor.IStandaloneCodeEditor, conflicts: ConflictBlock[]): ZoneRec[] {
  const recs: ZoneRec[] = [];
  ed.changeViewZones((accessor) => {
    for (const c of conflicts) {
      const dom = makeSpacerDom();
      const id = accessor.addZone({
        afterLineNumber: c.startLine - 1,
        heightInPx: 22,
        domNode: dom
      });
      recs.push({ id, dom });
    }
  });
  return recs;
}

const RO_OPTIONS = {
  readOnly: true,
  minimap: { enabled: false },
  scrollBeyondLastLine: false,
  fontSize: 12,
  lineNumbers: "on" as const,
  wordWrap: "on" as const,
  wrappingStrategy: "advanced" as const
};

interface MergeConflictEditorProps {
  file: FileStatus;
  onResolved: () => void;
}

export function MergeConflictEditor({ file, onResolved }: MergeConflictEditorProps) {
  const { isLocalMode } = useAuth();
  const { project, branchName } = useCurrentProjectBranch();
  const pathb64 = encodeBase64(file.path);
  const language = getLanguageFromFileName(file.path);

  const { data: rawContent = "", isLoading } = useQuery({
    queryKey: ["file-current", project.id, branchName, file.path],
    queryFn: () => FileService.getFile(project.id, pathb64, branchName),
    retry: false
  });

  const [result, setResult] = useState("");
  const [isSaving, setIsSaving] = useState(false);
  const [isSavingDraft, setIsSavingDraft] = useState(false);

  useEffect(() => {
    if (rawContent && result === "") setResult(rawContent);
  }, [rawContent, result]);

  const { text: incomingText, ranges: incomingRanges } = useMemo(
    () => resolveSideWithRanges(rawContent, "theirs"),
    [rawContent]
  );
  const { text: currentText, ranges: currentRanges } = useMemo(
    () => resolveSideWithRanges(rawContent, "mine"),
    [rawContent]
  );

  const conflicts = useMemo(() => parseConflicts(result), [result]);
  const stillHasMarkers = hasMarkers(result);
  const fileName = file.path.split("/").pop() ?? file.path;

  const incomingRef = useRef<editor.IStandaloneCodeEditor | null>(null);
  const currentRef = useRef<editor.IStandaloneCodeEditor | null>(null);
  const resultRef = useRef<editor.IStandaloneCodeEditor | null>(null);

  const incomingDecRef = useRef<DecIds>([]);
  const currentDecRef = useRef<DecIds>([]);
  const resultDecRef = useRef<DecIds>([]);
  const resultZonesRef = useRef<ZoneRec[]>([]);

  const [_incomingReady, setIncomingReady] = useState(false);
  const [_currentReady, setCurrentReady] = useState(false);
  const [resultReady, setResultReady] = useState(false);

  const [barPositions, setBarPositions] = useState<{ blockIndex: number; top: number }[]>([]);

  const saveDraftRef = useRef<(() => void) | undefined>(undefined);

  const handleConflictAction = useCallback((blockIndex: number, action: ConflictAction) => {
    setResult((prev) => resolveBlock(prev, blockIndex, action));
  }, []);

  // ViewZone at `c.startLine - 1` occupies the 22px before `c.startLine`;
  // `getTopForLineNumber` already accounts for view zones, so the bar's
  // top = getTopForLineNumber(start) - 22 - scrollTop.
  const computeBarPositions = useCallback(() => {
    const ed = resultRef.current;
    if (!ed) return;
    const scrollTop = ed.getScrollTop();
    setBarPositions(
      conflicts.map((c) => ({
        blockIndex: c.index,
        top: ed.getTopForLineNumber(c.startLine) - 22 - scrollTop
      }))
    );
  }, [conflicts]);

  const handleSaveDraft = useCallback(async () => {
    if (!project?.id || !branchName || isSavingDraft) return;
    setIsSavingDraft(true);
    try {
      await FileService.saveFile(project.id, pathb64, result, branchName);
    } catch {
      toast.error("Failed to save file");
    } finally {
      setIsSavingDraft(false);
    }
  }, [project?.id, branchName, pathb64, result, isSavingDraft]);

  useEffect(() => {
    // `handleSaveDraft` catches a failed save and toasts it, so the promise carries nothing.
    saveDraftRef.current = () => {
      void handleSaveDraft();
    };
  }, [handleSaveDraft]);

  const scrollSyncRef = useRef<{ dispose: () => void } | null>(null);

  const setupScrollSync = useCallback(() => {
    const inc = incomingRef.current;
    const cur = currentRef.current;
    if (!inc || !cur) return;
    scrollSyncRef.current?.dispose();
    let syncing = false;
    const d1 = inc.onDidScrollChange((e) => {
      if (syncing) return;
      syncing = true;
      cur.setScrollPosition({
        scrollTop: e.scrollTop,
        scrollLeft: e.scrollLeft
      });
      syncing = false;
    });
    const d2 = cur.onDidScrollChange((e) => {
      if (syncing) return;
      syncing = true;
      inc.setScrollPosition({
        scrollTop: e.scrollTop,
        scrollLeft: e.scrollLeft
      });
      syncing = false;
    });
    scrollSyncRef.current = {
      dispose: () => {
        d1.dispose();
        d2.dispose();
      }
    };
  }, []);

  useEffect(() => () => scrollSyncRef.current?.dispose(), []);

  useEffect(() => {
    const ed = resultRef.current;
    if (!ed || !resultReady) return;
    computeBarPositions();
    const d1 = ed.onDidScrollChange(computeBarPositions);
    const d2 = ed.onDidLayoutChange(computeBarPositions);
    return () => {
      d1.dispose();
      d2.dispose();
    };
  }, [resultReady, computeBarPositions]);

  useEffect(() => {
    injectStyles();
  }, []);

  useEffect(() => {
    const ed = incomingRef.current;
    if (!ed) return;
    incomingDecRef.current = applyHighlightDecs(
      ed,
      incomingRanges,
      "cmx-hl-theirs",
      incomingDecRef.current
    );
  }, [incomingRanges]);

  useEffect(() => {
    const ed = currentRef.current;
    if (!ed) return;
    currentDecRef.current = applyHighlightDecs(
      ed,
      currentRanges,
      "cmx-hl-mine",
      currentDecRef.current
    );
  }, [currentRanges]);

  useEffect(() => {
    const ed = resultRef.current;
    if (!ed) return;
    resultDecRef.current = applyResultDecs(ed, conflicts, resultDecRef.current);
  }, [conflicts]);

  useEffect(() => {
    const ed = resultRef.current;
    if (!ed) return;
    clearZones(ed, resultZonesRef.current);
    resultZonesRef.current = injectZones(ed, conflicts);
    // Recompute overlay positions after zones reinject (async layout).
    setTimeout(computeBarPositions, 0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [computeBarPositions, conflicts]);

  useEffect(() => {
    return () => {
      const ed = resultRef.current;
      if (ed) clearZones(ed, resultZonesRef.current);
    };
  }, []);

  const handleJumpNext = () => {
    const ed = resultRef.current;
    if (!ed) return;
    const model = ed.getModel();
    if (!model) return;
    const cur = ed.getPosition()?.lineNumber ?? 0;
    const total = model.getLineCount();
    for (let pass = 0; pass < 2; pass++) {
      const start = pass === 0 ? cur + 1 : 1;
      const end = pass === 0 ? total : cur;
      for (let i = start; i <= end; i++) {
        if (model.getLineContent(i).startsWith("<<<<<<<")) {
          ed.revealLineInCenter(i);
          ed.setPosition({ lineNumber: i, column: 1 });
          ed.focus();
          return;
        }
      }
    }
  };

  const handleSave = async () => {
    if (!project?.id || !branchName || stillHasMarkers) return;
    setIsSaving(true);
    try {
      const res = await ProjectService.resolveConflictWithContent(
        project.id,
        branchName,
        file.path,
        result
      );
      if (res.success) {
        onResolved();
      } else {
        toast.error("Failed to resolve conflict", {
          action: res.message
            ? {
                label: "Show details",
                onClick: () => toast.message(res.message)
              }
            : undefined
        });
      }
    } catch {
      toast.error("Failed to resolve conflict");
    } finally {
      setIsSaving(false);
    }
  };

  if (isLocalMode) return null;

  if (isLoading) {
    return (
      <div className='flex h-full items-center justify-center'>
        <Spinner className='text-muted-foreground' />
      </div>
    );
  }

  return (
    <div className='flex h-full flex-col'>
      {/* ── Toolbar ── */}
      <div className='flex shrink-0 items-center gap-1.5 border-border/40 border-b bg-sidebar-background px-3 py-1.5'>
        <span className='min-w-0 truncate font-mono text-[11px] text-muted-foreground/60'>
          {fileName}
        </span>

        <div className='ml-auto flex items-center gap-1.5'>
          {/* Conflict status */}
          {stillHasMarkers ? (
            <span className='flex items-center gap-1 font-mono text-[10px] text-warning/80'>
              <AlertTriangle className='h-3 w-3' />
              {conflicts.length} remaining
            </span>
          ) : (
            <span className='flex items-center gap-1 font-mono text-[10px] text-success/80'>
              <CheckCircle2 className='h-3 w-3' />
              all resolved
            </span>
          )}

          <div className='mx-1 h-3 w-px bg-border/50' />

          {/* Reset */}
          {result !== rawContent && (
            <button
              type='button'
              onClick={() => setResult(rawContent)}
              title='Reset — restore original conflict markers'
              className='flex h-5 items-center gap-1 rounded border border-border/40 px-1.5 font-mono text-[10px] text-muted-foreground transition-colors hover:border-border hover:bg-accent/40 hover:text-foreground'
            >
              <RotateCcw className='h-3 w-3' />
              Reset
            </button>
          )}

          {/* Jump */}
          {stillHasMarkers && (
            <button
              type='button'
              onClick={handleJumpNext}
              title='Jump to next conflict'
              className='flex h-5 items-center gap-1 rounded border border-border/40 px-1.5 font-mono text-[10px] text-muted-foreground transition-colors hover:border-border hover:bg-accent/40 hover:text-foreground'
            >
              <ChevronsDown className='h-3 w-3' />
              Next
            </button>
          )}

          {stillHasMarkers && <div className='mx-1 h-3 w-px bg-border/50' />}

          {/* Global accept shortcuts */}
          <button
            type='button'
            onClick={() => setResult(resolveAll(rawContent, "mine"))}
            className='flex h-5 items-center rounded border border-success/30 bg-success/8 px-2 font-mono text-[10px] text-success transition-colors hover:border-success/50 hover:bg-success/15'
          >
            Accept Mine
          </button>
          <button
            type='button'
            onClick={() => setResult(resolveAll(rawContent, "both" as ConflictAction))}
            className='flex h-5 items-center rounded border border-vis-violet/30 bg-vis-violet/8 px-2 font-mono text-[10px] text-vis-violet transition-colors hover:border-vis-violet/50 hover:bg-vis-violet/15'
          >
            Accept Both
          </button>
          <button
            type='button'
            onClick={() => setResult(resolveAll(rawContent, "theirs"))}
            className='flex h-5 items-center rounded border border-info/30 bg-info/8 px-2 font-mono text-[10px] text-info transition-colors hover:border-info/50 hover:bg-info/15'
          >
            Accept Theirs
          </button>

          <div className='mx-1 h-3 w-px bg-border/50' />

          {/* Complete Merge */}
          <button
            type='button'
            onClick={handleSave}
            disabled={stillHasMarkers || isSaving}
            title={
              stillHasMarkers
                ? "Resolve all conflicts first"
                : "Mark as resolved and continue rebase"
            }
            className='flex h-5 items-center gap-1 rounded bg-gradient-to-b from-[var(--blue-500)] to-[var(--blue-600)] px-2.5 font-medium font-mono text-[10px] text-white shadow-sm transition-all hover:from-[var(--blue-400)] hover:to-[var(--blue-500)] disabled:opacity-40'
          >
            {isSaving && <Spinner className='size-2.5' />}
            {isSaving ? "Resolving…" : "Resolve Conflict"}
          </button>
        </div>
      </div>

      {/* ── Body ── */}
      <ResizablePanelGroup direction='vertical' className='min-h-0 flex-1'>
        {/* Top: Incoming | Current */}
        <ResizablePanel defaultSize={38} minSize={15}>
          <ResizablePanelGroup direction='horizontal' className='h-full'>
            {/* Incoming (theirs) */}
            <ResizablePanel defaultSize={50} className='flex min-h-0 flex-col'>
              <div className='flex shrink-0 items-center gap-1.5 border-warning/15 border-b bg-warning/[0.03] px-3 py-1 font-mono text-[10px] text-warning/60'>
                <span className='font-semibold'>Incoming</span>
                <span className='rounded bg-warning/10 px-1 py-0.5 text-[9px] text-warning/50'>
                  theirs
                </span>
              </div>
              <div className='min-h-0 flex-1'>
                <ErrorBoundary
                  resetKeys={[file.path]}
                  fallback={
                    <ErrorAlert
                      className='m-3'
                      title='Failed to render incoming editor'
                      message='The incoming side could not be displayed.'
                    />
                  }
                >
                  <BaseMonacoEditor
                    value={incomingText}
                    language={language}
                    path={`incoming:${file.path}`}
                    height='100%'
                    options={RO_OPTIONS}
                    onMount={(ed) => {
                      incomingRef.current = ed;
                      incomingDecRef.current = applyHighlightDecs(
                        ed,
                        incomingRanges,
                        "cmx-hl-theirs",
                        []
                      );
                      setupScrollSync();
                      setIncomingReady(true);
                    }}
                  />
                </ErrorBoundary>
              </div>
            </ResizablePanel>

            <ResizableHandle className='bg-border/40 hover:bg-border' />

            {/* Current (mine) */}
            <ResizablePanel defaultSize={50} className='flex min-h-0 flex-col'>
              <div className='flex shrink-0 items-center gap-1.5 border-info/15 border-b bg-info/[0.03] px-3 py-1 font-mono text-[10px] text-info/60'>
                <span className='font-semibold'>Current</span>
                <span className='rounded bg-info/10 px-1 py-0.5 text-[9px] text-info/50'>mine</span>
              </div>
              <div className='min-h-0 flex-1'>
                <ErrorBoundary
                  resetKeys={[file.path]}
                  fallback={
                    <ErrorAlert
                      className='m-3'
                      title='Failed to render current editor'
                      message='The current side could not be displayed.'
                    />
                  }
                >
                  <BaseMonacoEditor
                    value={currentText}
                    language={language}
                    path={`current:${file.path}`}
                    height='100%'
                    options={RO_OPTIONS}
                    onMount={(ed) => {
                      currentRef.current = ed;
                      currentDecRef.current = applyHighlightDecs(
                        ed,
                        currentRanges,
                        "cmx-hl-mine",
                        []
                      );
                      setupScrollSync();
                      setCurrentReady(true);
                    }}
                  />
                </ErrorBoundary>
              </div>
            </ResizablePanel>
          </ResizablePanelGroup>
        </ResizablePanel>

        <ResizableHandle className='bg-border/40 hover:bg-border' />

        {/* Result */}
        <ResizablePanel defaultSize={62} minSize={25} className='flex min-h-0 flex-col'>
          <div
            className={`flex shrink-0 items-center gap-1.5 border-b px-3 py-1 font-mono text-[10px] ${
              stillHasMarkers
                ? "border-warning/15 bg-warning/[0.03] text-warning/60"
                : "border-success/15 bg-success/[0.03] text-success/60"
            }`}
          >
            <span className='font-semibold'>Result</span>
            <span className='opacity-50'>
              {stillHasMarkers
                ? `${conflicts.length} conflict${conflicts.length !== 1 ? "s" : ""} remaining`
                : "ready to stage"}
            </span>
          </div>
          {/* Relative container: Monaco editor + React action-bar overlay */}
          <div className='relative min-h-0 flex-1'>
            <ErrorBoundary
              resetKeys={[file.path]}
              fallback={
                <ErrorAlert
                  className='m-3'
                  title='Failed to render merge result editor'
                  message='The merge result editor could not be displayed. Try refreshing the page.'
                />
              }
            >
              <BaseMonacoEditor
                value={result}
                onChange={(v) => setResult(v)}
                language={language}
                path={`result:${file.path}`}
                height='100%'
                options={{ ...RO_OPTIONS, readOnly: false }}
                onMount={(ed, m) => {
                  resultRef.current = ed;
                  const initial = parseConflicts(result);
                  resultDecRef.current = applyResultDecs(ed, initial, []);
                  resultZonesRef.current = injectZones(ed, initial);
                  ed.addCommand(m.KeyMod.CtrlCmd | m.KeyCode.KeyS, () => saveDraftRef.current?.());
                  setResultReady(true);
                }}
              />
            </ErrorBoundary>
            {/* Action bars rendered as a React overlay — completely outside
                Monaco's DOM so mouse events are never intercepted by Monaco. */}
            <div className='pointer-events-none absolute inset-0 overflow-hidden'>
              {barPositions.map(({ blockIndex, top }) => (
                <div
                  key={blockIndex}
                  className='pointer-events-auto absolute right-0 left-0 flex items-center gap-1 px-2.5'
                  style={{
                    top,
                    height: 22,
                    background: "color-mix(in srgb, var(--background) 72%, transparent)",
                    borderBottom: "1px solid color-mix(in srgb, var(--border) 50%, transparent)",
                    backdropFilter: "blur(4px)",
                    fontFamily: "monospace",
                    fontSize: 10
                  }}
                >
                  {(
                    [
                      {
                        label: "Accept Mine",
                        action: "mine" as ConflictAction,
                        bg: "color-mix(in srgb, var(--conflict-mine) 15%, transparent)",
                        hbg: "color-mix(in srgb, var(--conflict-mine) 28%, transparent)",
                        color: "var(--conflict-mine)"
                      },
                      {
                        label: "Accept Both",
                        action: "both" as ConflictAction,
                        bg: "color-mix(in srgb, var(--conflict-both) 15%, transparent)",
                        hbg: "color-mix(in srgb, var(--conflict-both) 28%, transparent)",
                        color: "var(--conflict-both)"
                      },
                      {
                        label: "Accept Theirs",
                        action: "theirs" as ConflictAction,
                        bg: "color-mix(in srgb, var(--conflict-theirs) 15%, transparent)",
                        hbg: "color-mix(in srgb, var(--conflict-theirs) 28%, transparent)",
                        color: "var(--conflict-theirs)"
                      },
                      {
                        label: "Ignore",
                        action: "ignored" as ConflictAction,
                        bg: "color-mix(in srgb, var(--conflict-ignore) 15%, transparent)",
                        hbg: "color-mix(in srgb, var(--conflict-ignore) 35%, transparent)",
                        color: "var(--conflict-ignore)"
                      }
                    ] as const
                  ).map(({ label, action, bg, hbg, color }) => (
                    <button
                      key={action}
                      type='button'
                      onClick={() => handleConflictAction(blockIndex, action)}
                      onMouseEnter={(e) => {
                        (e.currentTarget as HTMLButtonElement).style.background = hbg;
                      }}
                      onMouseLeave={(e) => {
                        (e.currentTarget as HTMLButtonElement).style.background = bg;
                      }}
                      style={{
                        height: 16,
                        padding: "0 7px",
                        borderRadius: 3,
                        border: "none",
                        cursor: "pointer",
                        fontFamily: "monospace",
                        fontSize: 10,
                        background: bg,
                        color,
                        transition: "background 0.1s"
                      }}
                    >
                      {label}
                    </button>
                  ))}
                  <span
                    style={{
                      marginLeft: "auto",
                      opacity: 0.25,
                      fontSize: 9,
                      letterSpacing: "0.05em"
                    }}
                  >
                    {blockIndex + 1} / {conflicts.length}
                  </span>
                </div>
              ))}
            </div>
          </div>
        </ResizablePanel>
      </ResizablePanelGroup>
    </div>
  );
}
