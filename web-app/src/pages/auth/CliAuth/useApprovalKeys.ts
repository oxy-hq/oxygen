import { useEffect } from "react";

/**
 * An Escape the person pressed themselves, once. A script's event is not trusted, and a held key
 * repeats: neither is a decision.
 */
export const isCancelKey = (event: Pick<KeyboardEvent, "key" | "isTrusted" | "repeat">): boolean =>
  event.key === "Escape" && event.isTrusted && !event.repeat;

/**
 * The approval's one key, heard anywhere on the page: Escape cancels, since that is the safe
 * direction. Left out while a request is in flight.
 *
 * No key approves. oxyc opens this page at a moment it chooses and the browser comes to the
 * front, so a chord typed into another app (Command+Enter sends a chat message, a comment, a
 * commit) could otherwise approve a credential unread. Approving takes a click, or the Approve
 * button focused on purpose and pressed the way any button is.
 */
const useApprovalKeys = (onCancel?: () => void): void => {
  useEffect(() => {
    if (!onCancel) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (!event.defaultPrevented && isCancelKey(event)) onCancel();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onCancel]);
};

export default useApprovalKeys;
