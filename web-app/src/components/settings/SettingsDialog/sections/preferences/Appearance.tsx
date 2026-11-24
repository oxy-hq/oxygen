import { Monitor, Moon, Sun } from "lucide-react";
import { cn } from "@/libs/shadcn/utils";
import { chooseThemeMode } from "@/stores/themeSync";
import useTheme, { type ThemeMode } from "@/stores/useTheme";

const MODES: { value: ThemeMode; label: string; hint: string; icon: typeof Sun }[] = [
  { value: "light", label: "Light", hint: "Bright surfaces", icon: Sun },
  { value: "dark", label: "Dark", hint: "Dimmed surfaces", icon: Moon },
  { value: "system", label: "System", hint: "Follow your OS", icon: Monitor }
];

/** The person's light/dark mode. Signed in to a server, it is theirs, not the
 *  browser's: every custom app follows it, and changing it in an app changes it
 *  here (`stores/themeSync.ts`). In local mode it stays with this browser. */
export default function Appearance() {
  const mode = useTheme((s) => s.mode);

  return (
    <div className='flex flex-col gap-6'>
      <div className='grid max-w-xl grid-cols-3 gap-3'>
        {MODES.map(({ value, label, hint, icon: Icon }) => (
          <button
            key={value}
            type='button'
            onClick={() => chooseThemeMode(value)}
            data-active={mode === value}
            data-testid={`appearance-mode-${value}`}
            className={cn(
              "flex flex-col items-center gap-2 rounded-lg border p-4 text-sm transition-colors",
              "hover:border-primary/40 hover:bg-accent/50",
              "data-[active=true]:border-primary data-[active=true]:bg-primary/5"
            )}
          >
            <Icon className='h-5 w-5 text-muted-foreground' />
            <span className='font-medium'>{label}</span>
            <span className='text-muted-foreground text-xs'>{hint}</span>
          </button>
        ))}
      </div>
    </div>
  );
}
