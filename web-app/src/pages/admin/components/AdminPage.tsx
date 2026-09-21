import type { ReactNode } from "react";
import { useLocation } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import { adminPageTitle } from "../AdminLayout/adminNav";

/**
 * The frame every admin page is built in: one width scale, one padding, one rhythm,
 * one header.
 *
 * Before this, each page hand-rolled its own wrapper. Nineteen pages had **twelve**
 * distinct spellings of `mx-auto max-w-… p-6 …` — `max-w-2xl`, `3xl`, `4xl`, `5xl`,
 * `6xl`, `7xl`, `[100rem]`, with `space-y-4`, `5`, `6`, `8`, some with `pb-20`, some
 * with `lg:py-8`, some with `lg:py-10` — so two tables of the same shape lined up
 * differently depending on which page you opened, and a new page's author had to pick
 * one of twelve precedents at random.
 *
 * Width is a role, not a measurement. Pick by what the page holds:
 *
 * | Width     | Max      | For |
 * | --------- | -------- | --- |
 * | `narrow`  | `3xl`    | a single form or a short settings list |
 * | `default` | `5xl`    | prose + a card or two |
 * | `wide`    | `7xl`    | a table an operator scans |
 * | `full`    | none     | a split pane or a browser that owns the viewport |
 */
export type AdminPageWidth = "narrow" | "default" | "wide" | "full";

const WIDTH: Record<AdminPageWidth, string> = {
  narrow: "max-w-3xl",
  default: "max-w-5xl",
  wide: "max-w-7xl",
  full: "max-w-none"
};

export const AdminPage = ({
  title,
  description,
  actions,
  width = "wide",
  className,
  bodyClassName,
  children,
  "data-testid": dataTestId
}: {
  /**
   * Leave unset on a fixed page: the title comes from the route map, the same source
   * the rail and the topbar read, so the three cannot disagree. Pass it only when the
   * page is *about* something the route can't name — an org, a user, a workspace.
   */
  title?: string;
  /** One or two lines on what this surface is for. Optional; most tables need none. */
  description?: ReactNode;
  /** Page-level controls: refresh, a live indicator, the primary create button. */
  actions?: ReactNode;
  width?: AdminPageWidth;
  className?: string;
  /** Rhythm override for a page whose body owns its own scrolling (`full`, mostly). */
  bodyClassName?: string;
  children: ReactNode;
  "data-testid"?: string;
}) => {
  const location = useLocation();
  const heading = title ?? adminPageTitle(location.pathname, location.search);

  return (
    <div
      data-testid={dataTestId}
      className={cn("mx-auto w-full p-6 lg:px-10 lg:py-8", WIDTH[width], className)}
    >
      <header className='mb-5 flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between'>
        <div className='min-w-0 space-y-1'>
          <h1 className='truncate font-semibold text-xl tracking-tight'>{heading}</h1>
          {description ? (
            <div className='max-w-2xl text-muted-foreground text-xs'>{description}</div>
          ) : null}
        </div>
        {actions ? (
          <div className='flex shrink-0 flex-wrap items-center gap-2'>{actions}</div>
        ) : null}
      </header>
      <div className={cn("space-y-5", bodyClassName)}>{children}</div>
    </div>
  );
};
