import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";

export function useCopyTimeout() {
  const [copied, setCopied] = useState(false);
  const timeoutRef = useRef<NodeJS.Timeout | null>(null);

  // Cleanup timeout on unmount
  useEffect(() => {
    return () => {
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current);
      }
    };
  }, []);

  const handleCopy = useCallback(async (content: string) => {
    try {
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current);
      }

      await navigator.clipboard.writeText(content);
      setCopied(true);

      timeoutRef.current = setTimeout(() => {
        setCopied(false);
        timeoutRef.current = null;
      }, 2000);

      return true;
    } catch (err) {
      // The browser refused the write (no permission, or the page lost focus). Without
      // this the button just does nothing, which reads as "copied".
      console.error("Failed to copy:", err);
      toast.error("Couldn't copy to the clipboard");
      return false;
    }
  }, []);

  return { copied, handleCopy };
}
