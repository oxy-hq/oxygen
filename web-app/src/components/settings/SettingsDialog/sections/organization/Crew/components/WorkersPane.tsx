import { HardHat, Loader2, Search, UserRoundPlus } from "lucide-react";
import { useMemo, useState } from "react";
import TableWrapper from "@/components/settings/components/TableWrapper";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Table, TableBody, TableHead, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import type { AppAccessSummary } from "@/types/appAccess";
import type { FrontlineWorker } from "@/types/frontline";
import { EnrolWorkerDialog } from "./EnrolWorkerDialog";
import { WorkerRow } from "./WorkerRow";

export function WorkersPane({
  orgId,
  apps,
  workers,
  isPending,
  isError
}: {
  orgId: string;
  apps: AppAccessSummary[];
  workers: FrontlineWorker[];
  isPending: boolean;
  isError: boolean;
}) {
  const [search, setSearch] = useState("");
  const [enrolOpen, setEnrolOpen] = useState(false);

  const appsById = useMemo(() => new Map(apps.map((app) => [app.id, app])), [apps]);

  const filtered = useMemo(() => {
    const q = search.toLowerCase().trim();
    if (!q) return workers;
    return workers.filter(
      (w) => w.name.toLowerCase().includes(q) || w.identifier.toLowerCase().includes(q)
    );
  }, [workers, search]);

  return (
    <div className='flex flex-col gap-3'>
      <div className='flex items-end justify-between gap-3'>
        <div>
          <h3 className='font-medium'>Workers</h3>
          <p className='text-muted-foreground text-xs'>
            Each one signs in with their PIN and sees only the apps listed here.
          </p>
        </div>
        <Button
          size='sm'
          className='gap-1.5'
          onClick={() => setEnrolOpen(true)}
          data-testid='settings-crew-enrol'
        >
          <UserRoundPlus className='h-4 w-4' />
          Enroll worker
        </Button>
      </div>

      {isPending ? (
        <div className='flex min-h-24 items-center justify-center'>
          <Loader2 className='h-4 w-4 animate-spin text-muted-foreground' />
          <span className='sr-only'>Loading workers</span>
        </div>
      ) : isError ? (
        <p className='py-8 text-center text-destructive text-sm'>Failed to load workers.</p>
      ) : workers.length === 0 ? (
        <div className='flex flex-col items-center gap-3 rounded-md border py-10 text-center'>
          <HardHat className='h-8 w-8 text-muted-foreground/30' />
          <p className='max-w-sm text-muted-foreground text-sm'>
            Crew sign in on an enrolled kiosk with a PIN — enroll the first worker to get started.
          </p>
          <Button
            size='sm'
            variant='outline'
            className='mt-1 gap-1.5'
            onClick={() => setEnrolOpen(true)}
            data-testid='settings-crew-enrol-empty'
          >
            <UserRoundPlus className='h-4 w-4' />
            Enroll worker
          </Button>
        </div>
      ) : (
        <>
          <div className='relative'>
            <Search className='absolute top-1/2 left-3 h-4 w-4 -translate-y-1/2 text-muted-foreground' />
            <Input
              placeholder='Search workers...'
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              className='pl-9'
            />
          </div>
          {filtered.length === 0 ? (
            <p className='rounded-md border py-8 text-center text-muted-foreground text-sm'>
              No workers match "{search}"
            </p>
          ) : (
            <TableWrapper>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead className='px-4'>Name</TableHead>
                    <TableHead className='px-4'>Identifier</TableHead>
                    <TableHead className='px-4'>Status</TableHead>
                    <TableHead className='px-4'>Apps</TableHead>
                    <TableHead className='px-4'>Works at</TableHead>
                    <TableHead className='px-4'>Enrolled</TableHead>
                    <TableHead className='w-12' />
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {filtered.map((worker) => (
                    <WorkerRow
                      key={worker.user_id}
                      worker={worker}
                      orgId={orgId}
                      apps={apps}
                      appsById={appsById}
                    />
                  ))}
                </TableBody>
              </Table>
            </TableWrapper>
          )}
        </>
      )}

      <EnrolWorkerDialog open={enrolOpen} onOpenChange={setEnrolOpen} orgId={orgId} apps={apps} />
    </div>
  );
}
