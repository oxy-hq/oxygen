import { Activity, ArrowUpCircle, Cpu, Wifi, WifiOff } from "lucide-react";
import type React from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import type { EdgeBoxSummary } from "@/services/api/cameras";

/**
 * Utilization tab body. Reads only fields the worker stamps onto
 * the Postgres `edge_boxes` row, so it works regardless of whether
 * the Airhouse stats backend is up. CPU/mem/disk live in the
 * Airhouse `oxy_cam_box_health` time-series — surfacing those
 * requires a reader endpoint that doesn't exist yet, so we render
 * an explicit "coming next" hint rather than a broken fetch.
 *
 * The signals shown here answer the operator's first questions
 * during an incident: is this box reaching the controller right
 * now, what's it shipping over the wire, is it on a compatible
 * worker build, and is an image update in flight.
 */
type Props = {
  box: EdgeBoxSummary;
};

const ONLINE_THRESHOLD_MS = 5 * 60 * 1000;

const UtilizationPanel: React.FC<Props> = ({ box }) => {
  const heartbeat = describeHeartbeat(box.last_seen_at);
  const bandwidth = describeBandwidth(box.bandwidth_5min_bytes, box.bandwidth_reported_at);
  const updatePending =
    box.target_image_tag != null && box.target_image_tag !== box.current_image_tag;

  return (
    <div className='flex flex-col gap-3'>
      <div className='grid grid-cols-1 gap-3 md:grid-cols-3'>
        <Card
          icon={heartbeat.online ? <Wifi className='h-4 w-4' /> : <WifiOff className='h-4 w-4' />}
          label='Heartbeat'
          tone={heartbeat.online ? "default" : "danger"}
        >
          <div className='font-semibold text-foreground text-sm'>{heartbeat.headline}</div>
          {heartbeat.detail && (
            <div className='text-muted-foreground text-xs'>{heartbeat.detail}</div>
          )}
        </Card>

        <Card icon={<Activity className='h-4 w-4' />} label='Outbound bandwidth (5min)'>
          <div className='font-semibold text-foreground text-sm'>{bandwidth.headline}</div>
          {bandwidth.detail && (
            <div className='text-muted-foreground text-xs'>{bandwidth.detail}</div>
          )}
        </Card>

        <Card icon={<Cpu className='h-4 w-4' />} label='Host stats'>
          <div className='font-semibold text-muted-foreground text-sm'>Not wired up yet</div>
          <div className='text-muted-foreground text-xs'>
            CPU / memory / GPU live in Airhouse — needs a reader endpoint.
          </div>
        </Card>
      </div>

      <div className='grid grid-cols-1 gap-3 md:grid-cols-2'>
        <Card label='Image update'>
          <div className='flex flex-col gap-1.5'>
            <div className='flex items-baseline gap-2'>
              <code className='text-xs'>{box.current_image_tag ?? "(not reported)"}</code>
              {updatePending && (
                <Badge variant='outline' className='gap-1'>
                  <ArrowUpCircle className='h-3 w-3' />→ {box.target_image_tag}
                </Badge>
              )}
            </div>
            {box.last_update_result && (
              <div className='text-muted-foreground text-xs'>
                last result: <span className='text-foreground'>{box.last_update_result}</span>
                {box.last_update_at && ` · ${new Date(box.last_update_at).toLocaleString()}`}
              </div>
            )}
            {box.held_until && new Date(box.held_until) > new Date() && (
              <div className='text-muted-foreground text-xs'>
                hold window active until {new Date(box.held_until).toLocaleString()}
              </div>
            )}
          </div>
        </Card>

        <Card label='Worker compatibility'>
          {box.edge_compatibility_json ? (
            <div className='flex flex-col gap-0.5 text-xs'>
              {box.edge_compatibility_json.edge_version && (
                <div>
                  edge <code>v{box.edge_compatibility_json.edge_version}</code>
                </div>
              )}
              {box.edge_compatibility_json.protocol_version != null && (
                <div className='text-muted-foreground'>
                  protocol v{box.edge_compatibility_json.protocol_version}
                  {box.edge_compatibility_json.min_server_version &&
                    ` · min server ${box.edge_compatibility_json.min_server_version}`}
                </div>
              )}
              {box.edge_compatibility_json.git_sha && (
                <div className='font-mono text-muted-foreground'>
                  {box.edge_compatibility_json.git_sha.slice(0, 8)}
                  {box.edge_compatibility_json.built_at &&
                    ` · built ${new Date(box.edge_compatibility_json.built_at).toLocaleDateString()}`}
                </div>
              )}
              {box.incompatible_reason && (
                <div className='mt-1 border border-dot-warning/40 bg-dot-warning/10 px-2 py-1 text-status-warning'>
                  {box.incompatible_reason}
                </div>
              )}
            </div>
          ) : (
            <div className='text-muted-foreground text-xs'>
              No compatibility manifest reported. Box is likely on an older worker — operator should
              re-onboard or wait for the next update.
            </div>
          )}
        </Card>
      </div>

      <div className='grid grid-cols-1 gap-3 md:grid-cols-2'>
        <Card label='Tailscale IP'>
          <code className='break-all text-xs'>{box.tailscale_ip ?? "—"}</code>
        </Card>
        <Card label='Cohort'>
          <code className='text-xs'>{box.cohort || "—"}</code>
        </Card>
      </div>
    </div>
  );
};

const Card: React.FC<{
  label: string;
  icon?: React.ReactNode;
  tone?: "default" | "danger";
  children: React.ReactNode;
}> = ({ label, icon, tone = "default", children }) => {
  const toneClass =
    tone === "danger" ? "border-destructive/40 bg-destructive/5" : "border-border bg-muted/20";
  return (
    <div className={`flex flex-col gap-1.5 border p-3 ${toneClass}`}>
      <div className='flex items-center gap-1.5 text-[10px] text-muted-foreground uppercase tracking-wide'>
        {icon}
        <span>{label}</span>
      </div>
      {children}
    </div>
  );
};

function describeHeartbeat(lastSeenAt: string | null): {
  online: boolean;
  headline: string;
  detail: string | null;
} {
  if (!lastSeenAt) {
    return { online: false, headline: "Never reported", detail: null };
  }
  const last = new Date(lastSeenAt);
  const ageMs = Date.now() - last.getTime();
  const online = ageMs < ONLINE_THRESHOLD_MS;
  return {
    online,
    headline: online ? "Online" : "Stale",
    detail: `last ping ${formatAge(ageMs)} ago · ${last.toLocaleString()}`
  };
}

function describeBandwidth(
  bytes: number | null,
  reportedAt: string | null
): { headline: string; detail: string | null } {
  if (bytes == null) {
    return { headline: "—", detail: "no MTX bandwidth sample yet" };
  }
  return {
    headline: formatBytes(bytes),
    detail: reportedAt ? `reported ${new Date(reportedAt).toLocaleTimeString()}` : null
  };
}

function formatBytes(b: number): string {
  if (b < 1024) return `${b} B`;
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)} KB`;
  if (b < 1024 * 1024 * 1024) return `${(b / 1024 / 1024).toFixed(1)} MB`;
  return `${(b / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

function formatAge(ms: number): string {
  const seconds = Math.floor(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  return `${days}d`;
}

export default UtilizationPanel;
