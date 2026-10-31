import { cva } from "class-variance-authority";

export const buttonVariants = cva(
  `inline-flex items-center justify-center gap-2 t-button whitespace-nowrap rounded-md transition-[color,background-color,border-color,box-shadow,opacity,scale] duration-(--duration-fast) active:duration-(--duration-instant) disabled:pointer-events-none disabled:opacity-20 disabled:saturate-50 [&_svg]:pointer-events-none [&_svg:not([class*='size-'])]:size-4 shrink-0 [&_svg]:shrink-0 outline-none focus-visible:border-ring focus-visible:ring-ring/50 focus-visible:ring-[3px] aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive text-primary-foreground`,
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground shadow-xs hover:bg-primary/90",
        destructive:
          "bg-destructive text-white shadow-xs hover:bg-destructive/90 focus-visible:ring-destructive/20 dark:focus-visible:ring-destructive/40",
        outline: "border border-input text-foreground shadow-xs hover:bg-accent",
        secondary: "bg-secondary text-secondary-foreground shadow-xs hover:bg-secondary/80",
        ghost: "text-foreground hover:bg-accent",
        link: "text-primary underline-offset-4 hover:underline",
        white:
          "border border-input bg-background text-foreground shadow-xs hover:bg-accent focus-visible:ring-ring/50"
      },
      size: {
        default: "h-9 px-4 py-2",
        sm: "h-8 gap-1 rounded-md px-3",
        lg: "h-10 rounded-md px-6",
        icon: "size-9"
      }
    },
    defaultVariants: {
      variant: "default",
      size: "default"
    }
  }
);
