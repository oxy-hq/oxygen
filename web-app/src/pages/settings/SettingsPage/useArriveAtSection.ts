import { useEffect } from "react";
import type { SettingsSection } from "@/components/settings/settingsRoute";

/**
 * Puts a newly opened section where a new page should start: the pane at the top, focus on
 * its heading.
 *
 * A section change is a navigation, and the browser does neither for an in-page route swap —
 * it keeps the previous section's scroll, so you land mid-page, and leaves focus on the row
 * you clicked, so nothing is announced (WCAG 4.1.3).
 */
export function useArriveAtSection(
  section: SettingsSection | null,
  refs: {
    scrollRef: React.RefObject<HTMLDivElement | null>;
    headingRef: React.RefObject<HTMLHeadingElement | null>;
  }
) {
  const { scrollRef, headingRef } = refs;
  useEffect(() => {
    if (!section) return;
    scrollRef.current?.scrollTo({ top: 0 });
    headingRef.current?.focus({ preventScroll: true });
  }, [section, scrollRef, headingRef]);
}
