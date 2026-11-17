import { Monitor, Moon, Sun } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import { cn } from "@/libs/shadcn/utils";
import { chooseThemeMode } from "@/stores/themeSync";
import useTheme, { type ThemeMode } from "@/stores/useTheme";

const MODE_OPTIONS: { value: ThemeMode; label: string; icon: typeof Sun }[] = [
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
  { value: "system", label: "System", icon: Monitor }
];

const TRIGGER_ICON: Record<ThemeMode, typeof Sun> = {
  light: Sun,
  dark: Moon,
  system: Monitor
};

interface ThemeToggleProps {
  className?: string;
  align?: "start" | "center" | "end";
  side?: "top" | "right" | "bottom" | "left";
}

export function ThemeToggle({ className, align = "end", side = "top" }: ThemeToggleProps) {
  const mode = useTheme((state) => state.mode);
  const TriggerIcon = TRIGGER_ICON[mode];

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant='ghost'
          size='icon'
          aria-label='Toggle theme'
          className={cn("h-8 w-8", className)}
        >
          <TriggerIcon className='h-4 w-4' />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align={align} side={side} className='min-w-36'>
        {MODE_OPTIONS.map(({ value, label, icon: Icon }) => (
          <DropdownMenuItem
            key={value}
            onClick={() => chooseThemeMode(value)}
            className={cn("cursor-pointer", mode === value && "bg-muted")}
          >
            <Icon className='h-4 w-4' />
            <span>{label}</span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
