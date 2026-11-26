import type { LucideIcon } from "lucide-react";
import { cn } from "@/libs/shadcn/utils";

interface PageHeaderProps {
  icon?: LucideIcon;
  iconNode?: React.ReactNode;
  title: React.ReactNode;
  actions?: React.ReactNode;
  className?: string;
}

export default function PageHeader({
  icon: Icon,
  iconNode,
  title,
  actions,
  className
}: PageHeaderProps) {
  return (
    <div
      className={cn(
        "flex items-center justify-between border-border-subtle border-b bg-background px-4 py-1",
        className
      )}
    >
      <div className='flex min-h-8 items-center gap-3'>
        {iconNode ?? (Icon && <Icon className='h-4 w-4 text-subtle-foreground' />)}
        <h1 className='font-semibold text-sm'>{title}</h1>
      </div>
      {actions && <div className='flex items-center gap-3'>{actions}</div>}
    </div>
  );
}
