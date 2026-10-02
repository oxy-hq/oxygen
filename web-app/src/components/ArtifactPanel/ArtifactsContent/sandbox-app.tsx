import { ExternalLink, Maximize, Minimize, RotateCw, Terminal, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/shadcn/alert";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { apiBaseURL } from "@/services/env";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import type { SandboxAppArtifact } from "@/types/artifact";

type Props = {
  artifact: SandboxAppArtifact;
  /**
   * Optional API key to provide to iframe via postMessage.
   * If not provided, will attempt to read from localStorage 'oxy_api_key'
   */
  apiKey?: string;
};

interface V0LogMessage {
  currentURL: string;
  error: string;
  isFatal?: boolean;
  isServer: boolean;
  stack: string;
  type: "error";
  __v0_remote__: number;
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const isV0LogMessage = (obj: any): obj is V0LogMessage => {
  return obj && obj.type === "error" && typeof obj.__v0_remote__ === "number";
};

const v0ToConsoleLog = (v0Log: V0LogMessage): ConsoleLog => {
  const levelMap: { [key: string]: ConsoleLog["level"] } = {
    debug: "log",
    info: "info",
    warn: "warn",
    error: "error"
  };

  return {
    // eslint-disable-next-line sonarjs/pseudo-random
    id: `${Date.now()}-${Math.random()}`,
    timestamp: Date.now(),
    level: levelMap[v0Log.type] || "log",
    args: [v0Log.error, v0Log.stack, `(isServer: ${v0Log.isServer})`]
  };
};

type ConsoleLog = {
  id: string;
  timestamp: number;
  level: "log" | "info" | "warn" | "error";
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  args: any[];
};

const SandboxArtifactPanel = ({ artifact, apiKey }: Props) => {
  const [isLoading, setIsLoading] = useState(true);
  const [hasError, setHasError] = useState(false);
  const [isFullscreen, setIsFullscreen] = useState(false);
  const [showConsole, setShowConsole] = useState(false);
  const [consoleLogs, setConsoleLogs] = useState<ConsoleLog[]>([]);
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const consoleEndRef = useRef<HTMLDivElement>(null);

  const { preview_url } = artifact.content.value;
  const projectId = useCurrentWorkspace((state) => state.workspace?.id);

  const handleLoad = () => setIsLoading(false);
  const handleError = () => {
    setIsLoading(false);
    setHasError(true);
  };

  const handleRefresh = () => {
    if (iframeRef.current) {
      setIsLoading(true);
      iframeRef.current.src = preview_url;
    }
  };

  const handleFullscreen = () => {
    if (!containerRef.current) return;

    // Both calls reject when the browser denies them. There is nothing to show
    // the user: isFullscreen only follows `fullscreenchange`, so the button
    // keeps reflecting the state the panel is really in.
    if (!isFullscreen) {
      if (containerRef.current.requestFullscreen) {
        containerRef.current.requestFullscreen().catch((error: unknown) => {
          console.error("Sandbox preview could not enter fullscreen:", error);
        });
      }
    } else {
      if (document.exitFullscreen) {
        document.exitFullscreen().catch((error: unknown) => {
          console.error("Sandbox preview could not exit fullscreen:", error);
        });
      }
    }
  };

  // Set up postMessage listener for SDK authentication requests
  useEffect(() => {
    const handleAuthRequest = (event: MessageEvent) => {
      // Check if this is an Oxy SDK auth request
      if (event.data?.type !== "OXY_AUTH_REQUEST") {
        return;
      }

      if (!iframeRef.current || event.source !== iframeRef.current.contentWindow) {
        console.warn("[Oxy] Auth request from unknown source, ignoring");
        return;
      }

      // Validate iframe origin matches preview_url origin
      try {
        const previewOrigin = new URL(preview_url).origin;
        if (event.origin !== previewOrigin) {
          console.warn("[Oxy] Origin mismatch:", event.origin, "vs", previewOrigin);
          return;
        }
      } catch {
        console.error("[Oxy] Invalid preview URL:", preview_url);
        return;
      }

      const userApiKey = apiKey || localStorage.getItem("auth_token");

      if (!userApiKey) {
        console.error("[Oxy] No API key available to provide to iframe");
      }

      // Send authentication response to iframe
      if (event.source) {
        event.source.postMessage(
          {
            type: "OXY_AUTH_RESPONSE",
            version: "1.0",
            requestId: event.data.requestId,
            apiKey: userApiKey,
            projectId: projectId,
            baseUrl: apiBaseURL
          },
          event.origin
        );
      }
    };

    window.addEventListener("message", handleAuthRequest);

    // Cleanup on unmount
    return () => {
      window.removeEventListener("message", handleAuthRequest);
    };
  }, [preview_url, apiKey, projectId]);

  // Set up postMessage listener for console logs from iframe
  useEffect(() => {
    const handleConsoleLog = (event: MessageEvent) => {
      // Check if this is an Oxy console log message
      const data = event.data;
      if (!isV0LogMessage(data)) {
        return;
      }

      // Verify the message is from our iframe
      if (!iframeRef.current || event.source !== iframeRef.current.contentWindow) {
        return;
      }

      // Validate iframe origin matches preview_url origin
      try {
        const previewOrigin = new URL(preview_url).origin;
        if (event.origin !== previewOrigin) {
          return;
        }
      } catch {
        return;
      }

      const log: ConsoleLog = v0ToConsoleLog(data);

      setConsoleLogs((prev) => [...prev, log]);
    };

    window.addEventListener("message", handleConsoleLog);

    return () => {
      window.removeEventListener("message", handleConsoleLog);
    };
  }, [preview_url]);

  // Auto-scroll console to bottom when new logs arrive
  // biome-ignore lint/correctness/useExhaustiveDependencies: scroll on log changes
  useEffect(() => {
    if (showConsole && consoleEndRef.current) {
      consoleEndRef.current.scrollIntoView({ behavior: "smooth" });
    }
  }, [consoleLogs, showConsole]);

  // Listen for fullscreen changes
  useEffect(() => {
    const handleFullscreenChange = () => {
      setIsFullscreen(!!document.fullscreenElement);
    };

    document.addEventListener("fullscreenchange", handleFullscreenChange);
    return () => {
      document.removeEventListener("fullscreenchange", handleFullscreenChange);
    };
  }, []);

  if (!preview_url) {
    return artifact.is_streaming ? (
      <div className='p-4'>
        <Alert>
          {/* Streaming state with loader */}
          <AlertTitle>
            <div className='mb-2 flex items-center gap-2'>
              <Spinner className='size-6 text-muted-foreground' />
            </div>
          </AlertTitle>
          <AlertDescription>
            The sandbox preview is being generated. Please wait a moment.
          </AlertDescription>
        </Alert>
      </div>
    ) : (
      <div className='p-4'>
        <Alert variant='destructive'>
          <AlertTitle>Invalid Sandbox</AlertTitle>
          <AlertDescription>No preview URL available for this sandbox app.</AlertDescription>
        </Alert>
      </div>
    );
  }

  return (
    <div ref={containerRef} className='relative flex h-full flex-col'>
      {/* Loading overlay */}
      {isLoading && (
        <div className='absolute inset-0 z-10 flex items-center justify-center bg-background/50'>
          <Spinner className='size-8 text-muted-foreground' />
        </div>
      )}

      {/* Error state */}
      {hasError && (
        <div className='p-4'>
          <ErrorAlert
            title='Failed to Load Preview'
            actions={<Button size='sm'>Open in new tab</Button>}
          />
        </div>
      )}

      {/* Iframe */}
      {!hasError && (
        <iframe
          ref={iframeRef}
          src={preview_url}
          className='h-full w-full border-0'
          title='Sandbox Preview'
          onLoad={handleLoad}
          onError={handleError}
          sandbox='allow-scripts allow-same-origin allow-forms allow-popups allow-popups-to-escape-sandbox'
          allow='accelerometer; camera; encrypted-media; geolocation; gyroscope; microphone'
        />
      )}

      {/* Omni bar with controls */}
      <div className='absolute top-2 right-2 z-20 flex gap-1 rounded-md border border-base-border bg-background/80 p-1 backdrop-blur'>
        {artifact.is_streaming && (
          <div className='flex items-center gap-2 px-2 text-muted-foreground text-sm'>
            <Spinner />
          </div>
        )}
        <button
          onClick={handleRefresh}
          className='rounded p-2 transition-colors hover:bg-muted disabled:cursor-not-allowed disabled:opacity-50'
          title='Refresh page'
          disabled={artifact.is_streaming}
        >
          <RotateCw className='h-4 w-4' />
        </button>
        <button
          onClick={() => setShowConsole(!showConsole)}
          className={`rounded p-2 transition-colors hover:bg-muted disabled:cursor-not-allowed disabled:opacity-50 ${
            showConsole ? "bg-muted" : ""
          }`}
          title={showConsole ? "Hide console" : "Show console"}
          disabled={artifact.is_streaming}
        >
          <Terminal className='h-4 w-4' />
        </button>
        <button
          onClick={handleFullscreen}
          className='rounded p-2 transition-colors hover:bg-muted disabled:cursor-not-allowed disabled:opacity-50'
          title={isFullscreen ? "Exit fullscreen" : "Enter fullscreen"}
          disabled={artifact.is_streaming}
        >
          {isFullscreen ? <Minimize className='h-4 w-4' /> : <Maximize className='h-4 w-4' />}
        </button>
        <a
          href={preview_url}
          target='_blank'
          rel='noopener noreferrer'
          className={`flex rounded p-2 transition-colors hover:bg-muted ${
            artifact.is_streaming ? "pointer-events-none cursor-not-allowed opacity-50" : ""
          }`}
          title='Open in new tab'
        >
          <ExternalLink className='h-4 w-4' />
        </a>
      </div>

      {/* Console Panel */}
      {showConsole && (
        <div className='absolute right-0 bottom-0 left-0 z-20 flex h-64 flex-col border-base-border border-t bg-background'>
          {/* Console Header */}
          <div className='flex items-center justify-between border-base-border border-b bg-muted/30 px-4 py-2'>
            <div className='flex items-center gap-2'>
              <Terminal className='h-4 w-4' />
              <span className='font-semibold text-sm'>Console</span>
              <span className='text-muted-foreground text-xs'>
                ({consoleLogs.length} {consoleLogs.length === 1 ? "log" : "logs"})
              </span>
            </div>
            <div className='flex items-center gap-1'>
              <button
                onClick={() => setConsoleLogs([])}
                className='rounded px-2 py-1 text-xs transition-colors hover:bg-muted'
                title='Clear console'
              >
                Clear
              </button>
              <button
                onClick={() => setShowConsole(false)}
                className='rounded p-1 transition-colors hover:bg-muted'
                title='Close console'
              >
                <X className='h-4 w-4' />
              </button>
            </div>
          </div>

          {/* Console Logs */}
          <div className='flex-1 space-y-1 overflow-y-auto p-2 font-mono text-xs'>
            {consoleLogs.length === 0 ? (
              <div className='py-4 text-center text-muted-foreground'>No console logs yet</div>
            ) : (
              consoleLogs.map((log) => {
                const levelColors = {
                  log: "text-foreground",
                  info: "text-info",
                  warn: "text-warning",
                  error: "text-destructive"
                };

                const levelBgColors = {
                  log: "bg-muted/30",
                  info: "bg-info/10",
                  warn: "bg-warning/10",
                  error: "bg-destructive/10"
                };

                return (
                  <div key={log.id} className={`rounded px-2 py-1 ${levelBgColors[log.level]}`}>
                    <div className='flex items-start gap-2'>
                      <span className='whitespace-nowrap text-[10px] text-muted-foreground'>
                        {new Date(log.timestamp).toLocaleTimeString()}
                      </span>
                      <span
                        className={`font-semibold text-[10px] uppercase ${levelColors[log.level]}`}
                      >
                        {log.level}
                      </span>
                      <span className={`flex-1 ${levelColors[log.level]}`}>
                        {log.args
                          .map((arg) =>
                            typeof arg === "object" ? JSON.stringify(arg, null, 2) : String(arg)
                          )
                          .join(" ")}
                      </span>
                    </div>
                  </div>
                );
              })
            )}
            <div ref={consoleEndRef} />
          </div>
        </div>
      )}
    </div>
  );
};

export default SandboxArtifactPanel;
