import type { ComponentProps, ReactNode } from "react";
import { useRef } from "react";
import { Textarea } from "@/components/ui/shadcn/textarea";
import { cn } from "@/libs/shadcn/utils";

interface HighlightTextareaProps extends ComponentProps<typeof Textarea> {
  highlight?: ReactNode;
  overlayClassName?: string;
}

export function HighlightTextarea({
  highlight,
  overlayClassName = "px-3 py-2 text-base md:text-sm",
  className,
  onScroll,
  ...props
}: HighlightTextareaProps) {
  const overlayRef = useRef<HTMLDivElement>(null);

  const handleScroll = (e: React.UIEvent<HTMLTextAreaElement>) => {
    if (overlayRef.current) {
      overlayRef.current.scrollTop = e.currentTarget.scrollTop;
      overlayRef.current.scrollLeft = e.currentTarget.scrollLeft;
    }
    onScroll?.(e);
  };

  return (
    <div className='relative'>
      {highlight && (
        <div
          ref={overlayRef}
          aria-hidden
          // The touch-phone field rule in styles/shadcn/index.css sizes this overlay with
          // the textarea, so the drawn text stays on top of the caret.
          data-slot='highlight-overlay'
          className={cn(
            "pointer-events-none absolute inset-0 overflow-hidden whitespace-pre-wrap break-words",
            overlayClassName
          )}
        >
          {highlight}
        </div>
      )}
      <Textarea
        className={cn(className, highlight && "text-transparent [caret-color:var(--foreground)]")}
        onScroll={highlight ? handleScroll : onScroll}
        {...props}
      />
    </div>
  );
}
