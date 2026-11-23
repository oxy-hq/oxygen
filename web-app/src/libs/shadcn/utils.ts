import { type ClassValue, clsx } from "clsx";
import { extendTailwindMerge } from "tailwind-merge";

// The reference design language's type scale (`styles/reference/theme.css`) adds
// `text-ref-*` font sizes. Unregistered, tailwind-merge reads them as text
// colours and drops one whenever a colour follows (`text-ref-sm text-muted-foreground`).
const twMerge = extendTailwindMerge({
  extend: {
    theme: { text: ["ref-2xl", "ref-xl", "ref-base", "ref-md", "ref-sm", "ref-xs", "ref-2xs"] }
  }
});

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
