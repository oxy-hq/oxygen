import { useSyncExternalStore } from "react";

/**
 * A phone upright (narrower than Tailwind's `sm`, 640px) or on its side (a
 * touch screen no taller than 500px). A tablet and a short desktop window are
 * neither: the first is wide, the second has a fine pointer.
 */
export const PHONE_LAYOUT_QUERY =
  "(max-width: 639.98px), (pointer: coarse) and (max-height: 500px)";

function phoneQuery(): MediaQueryList | null {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return null;
  return window.matchMedia(PHONE_LAYOUT_QUERY);
}

function subscribe(onChange: () => void): () => void {
  const query = phoneQuery();
  query?.addEventListener?.("change", onChange);
  return () => query?.removeEventListener?.("change", onChange);
}

const getSnapshot = () => phoneQuery()?.matches ?? false;
// No window (server render, a worker): the desktop layout, which a phone then
// replaces on its first client render.
const getServerSnapshot = () => false;

/**
 * True when the app should render its phone view. Re-renders when the answer
 * changes (rotating the phone, resizing the window):
 *
 *   const phone = usePhoneLayout();
 *   return phone ? <OrdersPhone {...data} /> : <OrdersDesktop {...data} />;
 */
export function usePhoneLayout(): boolean {
  return useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot);
}
