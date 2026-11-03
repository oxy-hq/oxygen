import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle
} from "@/components/ui/shadcn/sheet";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/shadcn/tabs";
import type { AppEnvironment } from "@/services/api/customApps";
import { relativeTime } from "../../../Activity/relativeTime";
import { Outbox } from "./components/Outbox";
import { PlanesTable } from "./components/PlanesTable";

/**
 * One non-production environment in full: every plane and its mode (with a reset
 * where one exists), and the outbox a `capture` environment keeps. Read-first —
 * modes are chosen with `oxyc env update`, whose refusals carry the fix.
 */
export const EnvironmentDrawer = ({
  appId,
  env,
  onClose
}: {
  appId: string;
  env: AppEnvironment | null;
  onClose: () => void;
}) => (
  <Sheet open={!!env} onOpenChange={(open) => !open && onClose()}>
    <SheetContent
      side='right'
      className='flex w-full flex-col gap-0 p-0 sm:max-w-lg'
      data-testid='admin-app-env-drawer'
    >
      {env && (
        <>
          <SheetHeader className='border-b px-4 py-3'>
            <SheetTitle className='font-mono text-sm'>{env.name}</SheetTitle>
            <SheetDescription className='text-xs'>
              {env.kind}
              {env.parent ? ` · from ${env.parent}` : ""}
              {env.fork_at ? ` · forked ${relativeTime(env.fork_at)}` : ""}
            </SheetDescription>
          </SheetHeader>
          <Tabs defaultValue='planes' className='flex min-h-0 flex-1 flex-col px-4 py-3'>
            <TabsList className='self-start'>
              <TabsTrigger
                value='planes'
                className='text-xs'
                data-testid='admin-app-env-tab-planes'
              >
                Planes
              </TabsTrigger>
              <TabsTrigger
                value='outbox'
                className='text-xs'
                data-testid='admin-app-env-tab-outbox'
              >
                Outbox
              </TabsTrigger>
            </TabsList>
            <TabsContent value='planes' className='min-h-0 overflow-y-auto'>
              <PlanesTable appId={appId} env={env} />
            </TabsContent>
            <TabsContent value='outbox' className='min-h-0 overflow-y-auto'>
              <Outbox appId={appId} env={env} />
            </TabsContent>
          </Tabs>
        </>
      )}
    </SheetContent>
  </Sheet>
);
