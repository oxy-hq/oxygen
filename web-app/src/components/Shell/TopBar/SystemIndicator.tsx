import useOnlineStatus from "@/hooks/useOnlineStatus";

/**
 * The top bar's connection signal: nothing while the browser is online, a red
 * "Offline" when it is not. It mirrors `navigator.onLine` only, so a permanent
 * green "Connected" would be a light that can't go out; it shows up only with
 * news. A real backend health ping can replace `useOnlineStatus` later without
 * touching consumers.
 */
export function SystemIndicator() {
  const online = useOnlineStatus();
  if (online) return null;
  return (
    <span
      data-testid='topbar-system'
      role='status'
      className='inline-flex items-center gap-1.5 text-destructive text-xs'
    >
      <span className='inline-flex size-1.5 rounded-full bg-destructive' />
      Offline
    </span>
  );
}
