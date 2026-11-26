import { cn } from "@/libs/shadcn/utils";

interface OrgAvatarProps {
  name: string;
  className?: string;
}

function getInitials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return "?";
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}

export function OrgAvatar({ name, className }: OrgAvatarProps) {
  return (
    <div
      className={cn(
        "flex size-7 shrink-0 items-center justify-center bg-primary font-semibold text-primary-foreground text-xs",
        className
      )}
    >
      {getInitials(name)}
    </div>
  );
}
