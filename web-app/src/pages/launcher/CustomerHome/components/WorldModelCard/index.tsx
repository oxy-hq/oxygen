import { ChevronRight } from "lucide-react";
import { useMemo } from "react";
import { Link } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import { possessive } from "@/libs/utils/possessive";
import type { WorldModel } from "@/types/worldModel";
import { HomePanel } from "../HomePanel";
import { previewLayout } from "./previewLayout";
import { WorldModelCounts } from "./WorldModelCounts";
import { WorldModelPreview } from "./WorldModelPreview";

/** The card's title, and the world model page's heading. */
export function worldModelTitle(orgName?: string) {
  return orgName ? `How ${possessive(orgName)} data fits together` : "How your data fits together";
}

/**
 * "How <org>'s data fits together": a small drawing of the world model (the
 * entities the semantic model defines and how they connect), its counts, and a link
 * to the read-only world model page.
 *
 * The home decides whether to show it (a model with at least one entity) and hands
 * it the model, because the row it sits in is laid out differently without it.
 */
export function WorldModelCard({
  model,
  title,
  to,
  half = false,
  className
}: {
  model: WorldModel;
  title: string;
  /** The card takes half the row (under two apps, or two of four columns) rather than two thirds. */
  half?: boolean;
  /** The read-only world model page. */
  to: string;
  className?: string;
}) {
  // Two drawings, one per width: four columns from `sm` up, two on a phone, where
  // four columns would run the labels into each other.
  const wide = useMemo(() => previewLayout(model), [model]);
  const narrow = useMemo(() => previewLayout(model, { maxColumns: 2, maxPerColumn: 5 }), [model]);

  return (
    <HomePanel
      className={cn(
        "group transition-colors focus-within:ring-2 focus-within:ring-ring hover:border-primary/50",
        className
      )}
      testId='home-world-model-card'
      title={
        // The title's link stretches over the whole card, so the card is one target.
        <Link
          to={to}
          className='after:absolute after:inset-0 after:content-[""] focus-visible:outline-none'
          data-testid='home-world-model-link'
        >
          {title}
        </Link>
      }
      right={
        <ChevronRight
          className='size-4 shrink-0 text-muted-foreground transition-colors group-hover:text-foreground'
          aria-hidden
        />
      }
    >
      <div className='flex flex-1 items-center px-6 py-4'>
        <WorldModelPreview layout={wide} labelChars={half ? 14 : 18} className='hidden sm:block' />
        <WorldModelPreview layout={narrow} labelChars={14} className='sm:hidden' />
      </div>
      <WorldModelCounts model={model} className='border-t px-4 py-3' />
    </HomePanel>
  );
}
