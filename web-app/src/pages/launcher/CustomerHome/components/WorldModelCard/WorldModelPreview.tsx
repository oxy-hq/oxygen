import { cn } from "@/libs/shadcn/utils";
import type { PreviewLayout } from "./previewLayout";

function clip(label: string, max: number) {
  return label.length > max ? `${label.slice(0, max - 1)}…` : label;
}

/**
 * The home card's drawing of the world model: a dot per entity, a line per
 * connection. Positions are percentages, so the drawing stretches to the card
 * without scaling its text. Decorative for a screen reader, which gets the card's
 * own title and counts instead.
 */
export function WorldModelPreview({
  layout,
  labelChars,
  className
}: {
  layout: PreviewLayout;
  /** How many characters of a label fit beside a node at this drawing's width. */
  labelChars: number;
  className?: string;
}) {
  return (
    <svg
      className={cn("h-40 w-full overflow-visible text-xs", className)}
      aria-hidden='true'
      data-testid='home-world-model-preview'
    >
      {layout.edges.map((edge) => (
        <line
          key={edge.key}
          x1={`${edge.from.x}%`}
          y1={`${edge.from.y}%`}
          x2={`${edge.to.x}%`}
          y2={`${edge.to.y}%`}
          className='stroke-border'
          strokeWidth={1}
        />
      ))}
      {layout.nodes.map((node) => (
        <g key={node.id}>
          <circle
            cx={`${node.x}%`}
            cy={`${node.y}%`}
            r={4}
            className='fill-card stroke-foreground'
            strokeWidth={1.5}
          />
          <text
            x={`${node.x}%`}
            y={`${node.y}%`}
            dx={9}
            dy='0.35em'
            // A halo in the card's own colour, painted under the glyphs, so a line
            // passing behind a label never runs through its letters.
            className='fill-muted-foreground stroke-card'
            strokeWidth={4}
            paintOrder='stroke'
          >
            {clip(node.label, labelChars)}
          </text>
        </g>
      ))}
    </svg>
  );
}
