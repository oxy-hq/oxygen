import type React from "react";
import { cn } from "@/libs/shadcn/utils";

interface Props {
  /** An `<org>/<app>` reference, as oxyc spells it. */
  slug: string;
  /**
   * `strong` sets the app above its org, for a scope being granted. `quiet` sets the whole
   * reference back, for one not picked. `inherit` takes the colour and weight around it.
   */
  tone?: "strong" | "quiet" | "inherit";
  className?: string;
}

/**
 * An app named by its `<org>/<app>` reference, in monospace with the org set back: the same way
 * wherever a sandbox agent token's reach is shown.
 */
const AppSlug: React.FC<Props> = ({ slug, tone = "strong", className }) => {
  const cut = slug.indexOf("/") + 1;
  return (
    <span
      className={cn(
        "font-mono",
        tone === "strong" && "font-medium",
        tone === "quiet" && "text-muted-foreground",
        className
      )}
    >
      {tone === "strong" && cut > 0 ? (
        <>
          <span className='font-normal text-muted-foreground'>{slug.slice(0, cut)}</span>
          {slug.slice(cut)}
        </>
      ) : (
        slug
      )}
    </span>
  );
};

export default AppSlug;
