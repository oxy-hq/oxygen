import { AppArt } from "@/components/apps/AppArt";
import { AppMark } from "@/components/apps/AppMark";
import { cn } from "@/libs/shadcn/utils";
import type { CustomAppSummary } from "@/types/apps";

/**
 * The picture at the top of a home app card. An app with art shows it through
 * `AppArt`. An app without art gets its own mark (`AppMark`: its manifest icon, or
 * the monogram) centred on the same quiet tint `AppArt`'s fallback uses, at the
 * same size as its neighbours' art, so a row of cards keeps one height and the
 * card without art still reads as that app rather than as a missing image.
 *
 * Both pictures come only from the manifest (`art_url`, `icon_url`); see the
 * `oxy-app-visual-identity` skill.
 */
export function HomeAppArt({
  app,
  eager,
  compact,
  className
}: {
  app: CustomAppSummary;
  eager: boolean;
  /** The smaller mark, for the four-column grid. */
  compact: boolean;
  /** Sizing and edges: the card decides the aspect ratio and which side has a rule. */
  className: string;
}) {
  if (app.art_url) {
    return <AppArt artUrl={app.art_url} name={app.name} eager={eager} className={className} />;
  }
  return (
    <div
      className={cn("flex w-full items-center justify-center bg-primary/10", className)}
      data-testid={`home-app-card-placeholder-${app.slug}`}
    >
      <AppMark
        iconUrl={app.icon_url}
        name={app.name}
        size='lg'
        className={cn("bg-card", compact ? "size-10 text-base" : "size-14 text-xl")}
      />
    </div>
  );
}
