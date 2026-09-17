import { ArrowLeft, PanelLeft } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { usePublishApp, useUnpublishApp } from "@/hooks/api/customApps/useCustomApps";
import { cn } from "@/libs/shadcn/utils";
import type { CustomApp } from "@/types/apps";
import { AppDetail } from "../AppDetail";
import { RegistryRail } from "./RegistryRail";

interface AppCockpitProps {
  apps: CustomApp[];
  selected: CustomApp;
  onSelect: (app: CustomApp) => void;
  onBack: () => void;
}

/**
 * The selected-app cockpit: a persistent registry rail beside the live app
 * detail. Selecting a rail row swaps the stage without unmounting the shell, so
 * an operator can walk the fleet app-by-app with the keyboard and never lose the
 * preview's device/channel choices. The rail collapses to reclaim width; the
 * detail owns its own narrow-screen behavior (the dossier folds away below a
 * breakpoint), so nothing is ever clipped.
 *
 * Owns the publish/unpublish mutations so both the rail rows' hover actions and
 * the detail share one wiring.
 */
export const AppCockpit = ({ apps, selected, onSelect, onBack }: AppCockpitProps) => {
  const [railOpen, setRailOpen] = useState(true);
  const publishApp = usePublishApp();
  const unpublishApp = useUnpublishApp();
  const onPublish = (a: CustomApp) => publishApp.mutate(a.id);
  const onUnpublish = (a: CustomApp) => unpublishApp.mutate(a.id);

  return (
    <div className='flex h-full min-h-0 flex-col'>
      <header className='flex h-10 shrink-0 items-center gap-1 border-b px-2'>
        <Button variant='ghost' size='sm' className='h-8 gap-1.5' onClick={onBack}>
          <ArrowLeft className='size-3.5' />
          Apps
        </Button>
        <Button
          variant='ghost'
          size='icon'
          className={cn("size-8", railOpen && "text-foreground")}
          onClick={() => setRailOpen((o) => !o)}
          aria-label={railOpen ? "Hide registry" : "Show registry"}
          aria-pressed={railOpen}
          tooltip={{ content: railOpen ? "Hide registry" : "Show registry", side: "bottom" }}
        >
          <PanelLeft className='size-3.5' />
        </Button>
      </header>

      <div className='flex min-h-0 flex-1'>
        {/* Fixed-width collapsible rail — a plain flex aside rather than a nested
            resizable group, so the detail's own preview↔dossier split stays the
            only resizable surface (simpler, no imperative panel handles). */}
        <aside
          className={cn(
            "shrink-0 overflow-hidden border-r transition-[width] duration-200",
            railOpen ? "w-72" : "w-0"
          )}
        >
          <div className='h-full w-72'>
            <RegistryRail
              apps={apps}
              selected={selected}
              onSelect={onSelect}
              onPublish={onPublish}
              onUnpublish={onUnpublish}
            />
          </div>
        </aside>

        <div className='min-w-0 flex-1'>
          <AppDetail app={selected} />
        </div>
      </div>
    </div>
  );
};
