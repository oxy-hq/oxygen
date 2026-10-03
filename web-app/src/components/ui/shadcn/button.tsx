import { Slot } from "@radix-ui/react-slot";
import type { VariantProps } from "class-variance-authority";
import * as React from "react";

import { cn } from "@/libs/shadcn/utils";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "./tooltip";
import { buttonVariants } from "./utils/button-variants";

type TooltipConfig = {
  content: React.ReactNode;
  delayDuration?: number;
  sideOffset?: number;
} & Omit<React.ComponentProps<typeof TooltipContent>, "children">;

/** Whether `node` carries any text of its own, at any depth. An icon has none. */
const hasText = (node: React.ReactNode): boolean =>
  React.Children.toArray(node).some((child) => {
    if (typeof child === "string") return child.trim() !== "";
    if (typeof child === "number") return true;
    if (React.isValidElement<{ children?: React.ReactNode }>(child)) {
      return hasText(child.props.children);
    }
    return false;
  });

// `content` meets TooltipContent's HTML `content` attribute in TooltipConfig, so it
// is typed as a string either way.
const tooltipText = (tooltip: string | TooltipConfig | undefined) => {
  const content = typeof tooltip === "string" ? tooltip : tooltip?.content;
  return content?.trim() ? content : undefined;
};

const Button = React.forwardRef<
  HTMLButtonElement,
  React.ButtonHTMLAttributes<HTMLButtonElement> &
    VariantProps<typeof buttonVariants> & {
      asChild?: boolean;
      tooltip?: string | TooltipConfig;
    }
>(({ className, variant, size, asChild = false, tooltip, ...props }, ref) => {
  const Comp = asChild ? Slot : "button";

  // A tooltip is a hover description, not a name: an icon-only button would
  // otherwise reach a screen reader as just "button". Visible text stays the name.
  const namedElsewhere = props["aria-labelledby"] || hasText(props.children);
  const ariaLabel = props["aria-label"] ?? (namedElsewhere ? undefined : tooltipText(tooltip));

  const buttonElement = (
    <Comp
      ref={ref}
      data-slot='button'
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
      aria-label={ariaLabel}
    />
  );

  if (!tooltip) {
    return buttonElement;
  }

  // Handle string tooltips and object tooltips
  const {
    content,
    delayDuration = 300,
    ...tooltipProps
  } = typeof tooltip === "string" ? { content: tooltip } : tooltip;

  return (
    <TooltipProvider delayDuration={delayDuration}>
      <Tooltip>
        <TooltipTrigger asChild>{buttonElement}</TooltipTrigger>
        <TooltipContent {...tooltipProps}>{content}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
});

Button.displayName = "Button";

export { Button };
