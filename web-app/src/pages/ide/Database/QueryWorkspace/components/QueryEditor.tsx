import { get } from "lodash";
import { Code, Play, Plus, Save, X } from "lucide-react";
import type { editor } from "monaco-editor";
import { useCallback, useRef, useState } from "react";
import { ErrorBoundary } from "react-error-boundary";
import { toast } from "sonner";
import { format as formatSQL } from "sql-formatter";
import { BaseMonacoEditor, useMonacoSetup } from "@/components/MonacoEditor";
import DatabaseSelector from "@/components/sql/DatabaseSelector";
import ErrorAlert from "@/components/ui/ErrorAlert";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from "@/components/ui/shadcn/alert-dialog";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { buttonVariants } from "@/components/ui/shadcn/utils/button-variants";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { cn } from "@/libs/shadcn/utils";
import { DatabaseService } from "@/services/api";
import useDatabaseClient, { type SqlExecutionError } from "@/stores/useDatabaseClient";

interface QueryEditorProps {
  onSave: () => void;
}

/**
 * Pull a structured `SqlExecutionError` out of an axios error body, or return
 * `undefined` when the response doesn't carry one (network failures,
 * non-SQL endpoints).
 *
 * Uses index access on a `Record<string, unknown>` rather than lodash `get`
 * because lodash's typed `get` overload narrows untyped data to `never` when
 * fed an `unknown` input — fine at runtime, but the field reads below come
 * back as `never` and trip the type checker.
 */
function parseSqlExecutionError(error: unknown): SqlExecutionError | undefined {
  if (!error || typeof error !== "object") return undefined;
  const response = (error as Record<string, unknown>).response;
  if (!response || typeof response !== "object") return undefined;
  const data = (response as Record<string, unknown>).data;
  if (!data || typeof data !== "object") return undefined;
  const fields = data as Record<string, unknown>;

  const message = fields.message;
  if (typeof message !== "string" || message.length === 0) return undefined;

  return {
    message,
    code: typeof fields.code === "string" ? fields.code : undefined,
    detail: typeof fields.detail === "string" ? fields.detail : undefined,
    hint: typeof fields.hint === "string" ? fields.hint : undefined,
    position: typeof fields.position === "number" ? fields.position : undefined,
    sql: typeof fields.sql === "string" ? fields.sql : undefined
  };
}

export default function QueryEditor({ onSave }: QueryEditorProps) {
  const { project, branchName } = useCurrentProjectBranch();
  const {
    tabs,
    activeTabId,
    addTab,
    updateTab,
    removeTab,
    setActiveTab,
    getActiveTab,
    setTabExecuting,
    setTabResults,
    setTabError
  } = useDatabaseClient();

  const editorRef = useRef<editor.IStandaloneCodeEditor | null>(null);
  const [tabToClose, setTabToClose] = useState<{
    id: string;
    name: string;
  } | null>(null);

  const activeTab = tabs.find((t) => t.id === activeTabId);

  // Reads the active tab through `getActiveTab()` instead of closing over
  // `activeTab`: `updateTab` rebuilds `tabs` via `.map()` on every keystroke,
  // so a dep on the tab object would change this callback's identity on every
  // character — and `useMonacoSetup` disposes and re-registers the Cmd+Enter
  // command whenever `onExecute` changes. Every other dep here is stable.
  const handleRunQuery = useCallback(async () => {
    const tab = getActiveTab();
    if (!tab?.content.trim()) {
      toast.error("No query to execute");
      return;
    }

    if (!tab.selectedDatabase) {
      toast.error("Please select a database");
      return;
    }

    setTabExecuting(tab.id, true);
    const startTime = performance.now();

    try {
      const response = await DatabaseService.executeSqlQuery(
        project.id,
        branchName,
        tab.content,
        tab.selectedDatabase
      );

      const executionTime = performance.now() - startTime;

      // Handle parquet response (file reference)
      if (response && typeof response === "object" && "file_name" in response) {
        const truncated = response.truncated === true;
        toast.success(
          truncated
            ? `Query executed in ${executionTime.toFixed(0)}ms — showing first 10,000 rows`
            : `Query executed in ${executionTime.toFixed(0)}ms (results saved to file)`
        );
        setTabResults(tab.id, {
          result: [],
          resultFile: response.file_name,
          executionTime,
          truncated
        });
        return;
      }

      setTabResults(tab.id, {
        result: response as string[][],
        resultFile: undefined,
        executionTime
      });

      toast.success(`Query executed in ${executionTime.toFixed(0)}ms`);
    } catch (error) {
      const details = parseSqlExecutionError(error);
      const fallback =
        get(error, "response.data.error") || get(error, "message") || "Query execution failed";
      const message = details?.message ?? fallback;
      setTabError(tab.id, message, details);
    }
  }, [getActiveTab, project.id, branchName, setTabExecuting, setTabResults, setTabError]);

  // `handleRunQuery` catches its own failures (they land on the tab via `setTabError`),
  // so the promise carries nothing. Memoized for the same reason as above: a fresh
  // `onExecute` identity re-registers the Cmd+Enter command.
  const handleExecuteShortcut = useCallback(() => {
    void handleRunQuery();
  }, [handleRunQuery]);

  useMonacoSetup({ onSave, onExecute: handleExecuteShortcut });

  const handleNewTab = () => {
    const result = addTab({ selectedDatabase: activeTab?.selectedDatabase });
    if (!result.success) {
      toast.error(result.error);
    }
  };

  const handleCloseTab = (e: React.MouseEvent, tabId: string) => {
    e.stopPropagation();
    const tab = tabs.find((t) => t.id === tabId);
    if (tab?.isDirty) {
      setTabToClose({ id: tabId, name: tab.name });
      return;
    }
    removeTab(tabId);
  };

  const handleConfirmClose = () => {
    if (tabToClose) {
      removeTab(tabToClose.id);
      setTabToClose(null);
    }
  };

  const handleContentChange = (value: string) => {
    if (activeTab) {
      updateTab(activeTab.id, { content: value, isDirty: true });
    }
  };

  const handleFormatSQL = useCallback(() => {
    if (!activeTab?.content.trim()) {
      toast.error("No query to format");
      return;
    }

    try {
      const formatted = formatSQL(activeTab.content, {
        language: "sql",
        keywordCase: "upper",
        indentStyle: "standard",
        logicalOperatorNewline: "before"
      });
      updateTab(activeTab.id, { content: formatted, isDirty: true });
      toast.success("SQL formatted");
    } catch {
      toast.error("Failed to format SQL");
    }
  }, [activeTab, updateTab]);

  const handleEditorMount = (editor: editor.IStandaloneCodeEditor) => {
    editorRef.current = editor;
  };

  return (
    <div className='flex h-full flex-col bg-background'>
      <div className='flex items-center justify-between border-b px-2 py-1'>
        <div className='flex items-center gap-1'>
          <div className='mr-2'>
            <DatabaseSelector
              onSelect={(db) => activeTab && updateTab(activeTab.id, { selectedDatabase: db })}
              database={activeTab?.selectedDatabase ?? null}
            />
          </div>

          <Button
            variant='ghost'
            size='sm'
            onClick={handleRunQuery}
            disabled={!activeTab || activeTab.isExecuting || !activeTab?.selectedDatabase}
            className='h-7 px-2'
          >
            {activeTab?.isExecuting ? <Spinner /> : <Play className='h-4 w-4' />}
            <span>Run</span>
          </Button>
          <Button
            variant='ghost'
            size='sm'
            onClick={onSave}
            disabled={!activeTab}
            className='h-7 px-2'
          >
            <Save className='h-4 w-4' />
            <span>Save</span>
          </Button>
          <Button
            variant='ghost'
            size='sm'
            onClick={handleFormatSQL}
            disabled={!activeTab}
            className='h-7 px-2'
          >
            <Code className='h-4 w-4' />
            <span>Format</span>
          </Button>
        </div>

        {activeTab?.isExecuting && (
          <div className='text-muted-foreground text-xs'>
            <Spinner className='size-2.5' />
          </div>
        )}
      </div>

      {/* Tabs */}
      <div className='scrollbar-gutter-auto flex items-center overflow-x-auto border-b'>
        {tabs.map((tab) => (
          <div
            key={tab.id}
            onClick={() => setActiveTab(tab.id)}
            className={cn(
              "flex h-full shrink-0 cursor-pointer items-center gap-2 whitespace-nowrap border-r px-3 py-1.5 text-sm",
              "transition-colors hover:bg-muted/50",
              activeTabId === tab.id ? "border-b-2 border-b-primary bg-background" : "bg-muted/30"
            )}
          >
            <span className={cn(tab.isDirty && "italic")}>
              {tab.name}
              {tab.isDirty && " *"}
            </span>
            <Button
              variant='ghost'
              size='icon'
              className='h-4 w-4 hover:bg-muted/50'
              onClick={(e) => handleCloseTab(e, tab.id)}
            >
              <X className='h-3 w-3' />
            </Button>
          </div>
        ))}
        <Button variant='ghost' size='icon' className='mx-1 my-1 h-7 w-7' onClick={handleNewTab}>
          <Plus className='h-4 w-4' />
        </Button>
      </div>

      <div className='flex-1 overflow-hidden'>
        {activeTab ? (
          <ErrorBoundary
            resetKeys={[activeTab.id]}
            fallback={
              <ErrorAlert
                className='m-3'
                title='Failed to render SQL editor'
                message='The SQL editor could not be displayed. Try refreshing the page.'
              />
            }
          >
            <BaseMonacoEditor
              value={activeTab.content}
              onChange={handleContentChange}
              onMount={handleEditorMount}
              language='sql'
              options={{
                minimap: { enabled: false },
                scrollBeyondLastLine: true
              }}
            />
          </ErrorBoundary>
        ) : (
          <div className='flex h-full flex-col items-center justify-center text-muted-foreground'>
            <Code className='mb-4 h-12 w-12 opacity-30' />
            <p className='text-sm'>No query open</p>
            <Button variant='link' size='sm' onClick={handleNewTab}>
              Create new query
            </Button>
          </div>
        )}
      </div>

      <AlertDialog open={!!tabToClose} onOpenChange={(open) => !open && setTabToClose(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Unsaved Changes</AlertDialogTitle>
            <AlertDialogDescription>
              "{tabToClose?.name}" has unsaved changes. Are you sure you want to close it?
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className={buttonVariants({ variant: "destructive" })}
              onClick={handleConfirmClose}
            >
              Close
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
