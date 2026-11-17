import { type RefObject, useEffect, useState } from "react";

/**
 * `ref`'s element's content width in pixels, kept current as it resizes: the diagram
 * is laid out in pixels, and the console's columns change width with the window.
 * Content width, so a scrollbar the element shows is not counted as room. `0` until
 * measured (and in jsdom, which lays nothing out).
 */
export function useElementWidth(ref: RefObject<HTMLElement | null>): number {
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    setWidth(element.clientWidth);
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(([entry]) => {
      if (entry) setWidth(Math.floor(entry.contentRect.width));
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref]);
  return width;
}
