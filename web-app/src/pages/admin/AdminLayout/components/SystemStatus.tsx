import { useEffect, useState } from "react";
import { useAuth } from "@/contexts/AuthContext";
import { cn } from "@/libs/utils/cn";

/**
 * Command-deck status cluster for the admin topbar: which deployment you're
 * operating (LOCAL / CLOUD / REMOTE) and a live UTC clock. Small, monospace,
 * always-ticking — the ambient "this is a control surface" signal that makes
 * the admin shell read as a cockpit rather than a settings page.
 */
export function SystemStatus() {
  const { authConfig } = useAuth();
  const env = authConfig.mode === "cloud" ? "CLOUD" : "LOCAL";

  return (
    <div className='@5xl:flex hidden items-center gap-3'>
      <span
        className={cn(
          "flex items-center gap-1.5 rounded-sm border px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-[0.16em]",
          env === "CLOUD" ? "border-primary/40 text-primary" : "border-border text-muted-foreground"
        )}
        title={`Deployment mode: ${env}`}
      >
        <span
          className={cn(
            "size-1.5 rounded-full",
            env === "CLOUD" ? "bg-primary" : "bg-muted-foreground/60"
          )}
          aria-hidden
        />
        {env}
      </span>
      <UtcClock />
    </div>
  );
}

function UtcClock() {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const id = window.setInterval(() => setNow(new Date()), 1000);
    return () => window.clearInterval(id);
  }, []);
  const hh = String(now.getUTCHours()).padStart(2, "0");
  const mm = String(now.getUTCMinutes()).padStart(2, "0");
  const ss = String(now.getUTCSeconds()).padStart(2, "0");
  return (
    <span
      className='font-mono text-[11px] text-muted-foreground tabular-nums'
      title='Current time (UTC)'
    >
      {hh}:{mm}:{ss}
      <span className='ml-1 text-[9px] text-muted-foreground/50'>UTC</span>
    </span>
  );
}
