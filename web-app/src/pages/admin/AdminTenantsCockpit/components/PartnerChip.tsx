import { Handshake } from "lucide-react";
import { cn } from "@/libs/shadcn/utils";

/**
 * A partner shown inline on an org row / relationship strip. Clickable variant
 * filters the org list to that partner (the one-click "connections" view);
 * static variant is a plain label. Kept intentionally tiny for dense rows.
 */
export default function PartnerChip({
  name,
  onClick,
  size = "sm"
}: {
  name: string;
  onClick?: () => void;
  size?: "sm" | "xs";
}) {
  const cls = cn(
    "inline-flex max-w-full items-center gap-1 border border-primary/20 bg-primary/5 font-medium text-primary",
    size === "xs" ? "px-1 py-0 text-[10px]" : "px-1.5 py-0.5 text-xs",
    onClick && "cursor-pointer transition-colors hover:bg-primary/15"
  );

  const inner = (
    <>
      <Handshake className={size === "xs" ? "size-2.5 shrink-0" : "size-3 shrink-0"} />
      <span className='truncate'>{name}</span>
    </>
  );

  if (onClick)
    return (
      <button
        type='button'
        className={cls}
        onClick={(e) => {
          e.stopPropagation();
          onClick();
        }}
        title={`Show organizations managed by ${name}`}
      >
        {inner}
      </button>
    );
  return <span className={cls}>{inner}</span>;
}
