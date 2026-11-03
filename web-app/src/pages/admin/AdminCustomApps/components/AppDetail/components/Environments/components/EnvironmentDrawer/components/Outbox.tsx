import { Inbox } from "lucide-react";
import { useEnvironmentOutbox } from "@/hooks/api/customApps/useAppEnvironments";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { AdminEmptyState } from "@/pages/admin/components/AdminEmptyState";
import type { AppEnvironment, OutboxMessage } from "@/services/api/customApps";
import { relativeTime } from "../../../../Activity/relativeTime";

/**
 * What a `capture` environment kept instead of sending, newest first. Bodies are
 * shown as text, never rendered as HTML. An environment not on `capture` sends
 * its mail, so its outbox stays empty and says why.
 */
export const Outbox = ({ appId, env }: { appId: string; env: AppEnvironment }) => {
  const capturing = env.planes?.messages === "capture";
  const outbox = useEnvironmentOutbox(appId, env.name, true);
  return (
    <AdminAsync
      query={outbox}
      noun='outbox'
      rows={3}
      isEmpty={(d) => d.messages.length === 0}
      empty={
        <AdminEmptyState
          icon={Inbox}
          title='Nothing captured.'
          description={
            capturing
              ? "Email and notifications this environment sends will appear here."
              : "This environment sends its email (messages=invoker). Set messages=capture to keep it here instead."
          }
        />
      }
    >
      {(data) => (
        <ul className='flex flex-col gap-2' data-testid='admin-app-env-outbox'>
          {data.messages.map((m) => (
            <OutboxItem key={`${m.invocation_id}-${m.captured_at}`} message={m} />
          ))}
          {data.has_more && (
            <li className='text-muted-foreground text-xs'>
              Older messages: oxyc env outbox --limit 200
            </li>
          )}
        </ul>
      )}
    </AdminAsync>
  );
};

const recipients = (to: unknown): string => {
  if (typeof to === "string") return to;
  if (Array.isArray(to))
    return to.map((t) => (typeof t === "string" ? t : JSON.stringify(t))).join(", ");
  return to == null ? "nobody" : JSON.stringify(to);
};

const OutboxItem = ({ message }: { message: OutboxMessage }) => (
  <li
    className='flex flex-col gap-1 rounded-md border p-2 text-xs'
    data-testid='admin-app-env-outbox-item'
  >
    <div className='flex items-center gap-2'>
      <span className='font-medium'>{message.subject ?? message.title ?? "(no subject)"}</span>
      <span className='text-muted-foreground'>{message.kind}</span>
      <span className='ml-auto text-muted-foreground'>{relativeTime(message.captured_at)}</span>
    </div>
    <span className='text-muted-foreground'>
      to {recipients(message.to)} · from {message.function}
    </span>
    {message.body && (
      <pre className='max-h-48 overflow-auto whitespace-pre-wrap break-words rounded bg-muted p-2 font-mono text-xs'>
        {message.body}
      </pre>
    )}
  </li>
);
