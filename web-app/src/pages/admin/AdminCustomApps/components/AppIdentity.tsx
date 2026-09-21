import { cn } from "@/libs/shadcn/utils";
import type { CustomApp } from "@/types/apps";

/**
 * How a custom app is named, everywhere it is named.
 *
 * **An app's name does not identify it.** `oxy-starter` exists under `acme` and under
 * `local`, so the registry table used to show two rows both reading "Oxy Starter", told
 * apart only by a small Org cell three columns away. This component cannot render a name
 * without its org, which is the point: the switcher, the console header and the storage
 * audit all go through it, so none of them can reintroduce the ambiguity.
 *
 * The slug line is the copy-pasteable identity — `acme/oxy-starter` is what appears in a
 * URL, in `oxyc` commands and in a Slack message, so it is rendered in mono and is
 * selectable rather than being decoration.
 */
export const AppIdentity = ({
  app,
  size = "sm",
  className,
  "data-testid": dataTestId
}: {
  app: Pick<CustomApp, "name" | "slug" | "org_slug">;
  /** `sm` for list rows, `lg` for the console header. */
  size?: "sm" | "lg";
  className?: string;
  "data-testid"?: string;
}) => (
  <span className={cn("flex min-w-0 flex-col gap-0.5", className)} data-testid={dataTestId}>
    <span className='flex min-w-0 items-baseline gap-2'>
      <span
        className={cn(
          "truncate font-semibold tracking-tight",
          size === "lg" ? "text-xl" : "text-xs"
        )}
      >
        {app.name}
      </span>
      {/* The org is a peer of the name, not a distant column. At `lg` it keeps the
          smaller size on purpose — it qualifies the name rather than competing. */}
      <span className='shrink-0 truncate text-muted-foreground text-xs'>{app.org_slug}</span>
    </span>
    <span className='truncate font-mono text-[10px] text-muted-foreground'>
      {app.org_slug}/{app.slug}
    </span>
  </span>
);
