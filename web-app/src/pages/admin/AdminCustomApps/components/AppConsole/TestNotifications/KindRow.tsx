import { Loader2, Send } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import { useSendTestNotification } from "@/hooks/api/customApps/useAppNotificationTest";
import type { AppNotificationKind } from "@/types/apps";
import { testNotificationError } from "./testNotificationError";

const LEVEL_WORDS: Record<AppNotificationKind["level"], string | null> = {
  passive: "passive",
  active: null,
  time_sensitive: "time-sensitive",
  critical: "critical"
};

/** One kind: what a person sees it as, and a button that sends it to you. */
export const KindRow = ({
  appId,
  kind,
  disabled
}: {
  appId: string;
  kind: AppNotificationKind;
  disabled: boolean;
}) => {
  const send = useSendTestNotification();
  const level = LEVEL_WORDS[kind.level];
  const onSend = () =>
    send.mutate(
      { id: appId, kind: kind.kind },
      {
        onSuccess: () => toast.success(`Sent “${kind.label}” to you. Check your phone.`),
        onError: (e) => toast.error(testNotificationError(e))
      }
    );

  return (
    <li
      className='flex items-center justify-between gap-3 border border-border/60 px-2.5 py-1.5'
      data-testid={`admin-app-notification-test-kind-${kind.kind}`}
    >
      <div className='flex min-w-0 flex-col'>
        <span className='truncate font-medium text-xs'>{kind.label}</span>
        <span className='truncate text-muted-foreground text-xs'>
          <code>{kind.kind}</code>
          {level ? ` · ${level}` : null}
          {kind.mutable ? null : " · ignores mutes"}
        </span>
      </div>
      <Button
        type='button'
        variant='outline'
        size='sm'
        className='h-7 shrink-0 gap-1.5 text-xs'
        disabled={disabled || send.isPending}
        onClick={onSend}
        data-testid={`admin-app-notification-test-send-${kind.kind}`}
      >
        {send.isPending ? <Loader2 className='size-3 animate-spin' /> : <Send className='size-3' />}
        Send to me
      </Button>
    </li>
  );
};
