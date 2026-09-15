import { useEffect, useRef, useState } from "react";

const AUTO_COLLAPSE_DELAY_MS = 800;

function useAutoCollapse(isStreaming: boolean, hasSteps: boolean, defaultCollapsed = false) {
  const [collapsed, setCollapsed] = useState(defaultCollapsed);
  const wasStreamingRef = useRef(isStreaming);

  useEffect(() => {
    if (wasStreamingRef.current && !isStreaming && hasSteps) {
      const timer = setTimeout(() => setCollapsed(true), AUTO_COLLAPSE_DELAY_MS);
      wasStreamingRef.current = false;
      return () => clearTimeout(timer);
    }
    wasStreamingRef.current = isStreaming;
  }, [isStreaming, hasSteps]);

  return [collapsed, setCollapsed] as const;
}

export default useAutoCollapse;
