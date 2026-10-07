import { useEffect, useState } from "react";

/** How long the request must be in front of the person before Approve can be pressed. */
const ARM_DELAY_MS = 1000;

const attended = (): boolean => document.visibilityState === "visible" && document.hasFocus();

/**
 * Whether Approve may be pressed yet: there is a request to approve, and the page has been both
 * visible and focused for a full second since.
 *
 * oxyc opens this page at a moment it chooses, and the browser comes to the front. Without the
 * wait, a click or a key already on its way to another window could land on Approve. Hiding the
 * tab or leaving the window disarms it, and coming back counts the second again.
 *
 * @param ready There is something to approve. The second is counted from then, so Approve never
 *   turns on in the same instant the request appears.
 */
const useApprovalArmed = (ready: boolean): boolean => {
  const [armed, setArmed] = useState(false);

  useEffect(() => {
    if (!ready) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const count = () => {
      if (attended()) timer = setTimeout(() => setArmed(true), ARM_DELAY_MS);
    };
    const recount = () => {
      clearTimeout(timer);
      setArmed(false);
      count();
    };
    count();
    document.addEventListener("visibilitychange", recount);
    window.addEventListener("focus", recount);
    window.addEventListener("blur", recount);
    return () => {
      clearTimeout(timer);
      setArmed(false);
      document.removeEventListener("visibilitychange", recount);
      window.removeEventListener("focus", recount);
      window.removeEventListener("blur", recount);
    };
  }, [ready]);

  return ready && armed;
};

export default useApprovalArmed;
