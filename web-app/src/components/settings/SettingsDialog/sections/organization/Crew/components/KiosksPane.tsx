import { Ban, Loader2, Plus, QrCode } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import TableWrapper from "@/components/settings/components/TableWrapper";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from "@/components/ui/shadcn/alert-dialog";
import { Button } from "@/components/ui/shadcn/button";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import { useReissueEnrolLink, useRevokeDevice } from "@/hooks/api/organizations";
import { type KioskState, kioskState } from "@/libs/frontline";
import { timeAgo } from "@/libs/utils/date";
import type { AppAccessSummary } from "@/types/appAccess";
import type { CreatedKioskDevice, KioskDeviceRow } from "@/types/frontline";
import { appForReturnTo } from "../utils";
import { EnrolLinkDialog } from "./EnrolLinkDialog";
import { KioskIdleCell } from "./KioskIdleCell";
import { KioskStateBadge } from "./KioskStateBadge";
import { NewKioskDialog } from "./NewKioskDialog";

export function KiosksPane({
  orgId,
  orgSlug,
  apps,
  devices,
  isPending,
  isError
}: {
  orgId: string;
  orgSlug: string;
  apps: AppAccessSummary[];
  devices: KioskDeviceRow[];
  isPending: boolean;
  isError: boolean;
}) {
  const [creating, setCreating] = useState(false);
  // The link is held here, not in the query cache: it exists only in this
  // response and must not survive past the dialog that shows it.
  const [created, setCreated] = useState<CreatedKioskDevice | null>(null);
  const [pendingRevoke, setPendingRevoke] = useState<KioskDeviceRow | null>(null);
  const revokeDevice = useRevokeDevice();
  const reissueLink = useReissueEnrolLink();

  const handleReissue = (device: KioskDeviceRow) => {
    reissueLink.mutate(
      { orgId, deviceId: device.id },
      {
        onSuccess: setCreated,
        onError: () => toast.error(`Couldn't make a new link for ${device.name}`)
      }
    );
  };

  const handleRevoke = () => {
    if (!pendingRevoke) return;
    const device = pendingRevoke;
    revokeDevice.mutate(
      { orgId, deviceId: device.id },
      {
        onSuccess: () => {
          toast.success(`Revoked ${device.name}`);
          setPendingRevoke(null);
        },
        onError: () => toast.error("Couldn't revoke the kiosk")
      }
    );
  };

  return (
    <div className='flex flex-col gap-3'>
      <div className='flex items-end justify-between gap-3'>
        <div>
          <h3 className='font-medium'>Kiosks</h3>
          <p className='text-muted-foreground text-xs'>
            The tablets crew sign in on. A PIN only works on one of these.
          </p>
        </div>
        <Button
          size='sm'
          variant='outline'
          className='gap-1.5'
          onClick={() => setCreating(true)}
          data-testid='settings-crew-new-kiosk'
        >
          <Plus className='h-4 w-4' />
          New kiosk
        </Button>
      </div>

      {isPending ? (
        <div className='flex min-h-24 items-center justify-center'>
          <Loader2 className='h-4 w-4 animate-spin text-muted-foreground' />
          <span className='sr-only'>Loading kiosks</span>
        </div>
      ) : isError ? (
        <p className='py-8 text-center text-destructive text-sm'>Failed to load kiosks.</p>
      ) : devices.length === 0 ? (
        <p className='rounded-md border py-8 text-center text-muted-foreground text-sm'>
          No kiosks yet — enroll a tablet and crew can sign in on it.
        </p>
      ) : (
        <TableWrapper>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className='px-4'>Name</TableHead>
                <TableHead className='px-4'>Location</TableHead>
                <TableHead className='px-4'>State</TableHead>
                <TableHead className='px-4'>Opens</TableHead>
                <TableHead className='px-4'>Signs out</TableHead>
                <TableHead className='w-12' />
              </TableRow>
            </TableHeader>
            <TableBody>
              {devices.map((device) => {
                const state = kioskState(device);
                const app = appForReturnTo(apps, orgSlug, device.return_to);
                return (
                  <TableRow key={device.id} data-testid={`settings-crew-kiosk-${device.id}`}>
                    <TableCell data-label='Name' className='px-4 py-3 max-md:px-0 max-md:py-0'>
                      <span className='font-medium text-sm'>{device.name}</span>
                    </TableCell>
                    <TableCell
                      data-label='Location'
                      className='px-4 py-3 text-sm max-md:px-0 max-md:py-0'
                    >
                      {device.location_name ?? <span className='text-muted-foreground'>—</span>}
                    </TableCell>
                    <TableCell data-label='State' className='px-4 py-3 max-md:px-0 max-md:py-0'>
                      <div className='flex flex-col items-start gap-1'>
                        <KioskStateBadge state={state} />
                        <span className='text-muted-foreground text-xs'>
                          {stateDetail(state, device)}
                        </span>
                        {/* The link was shown once and only its hash is kept, so a
                            lost one is replaced — never for a bound kiosk. */}
                        {(state === "waiting" || state === "expired") && (
                          <Button
                            variant='link'
                            size='sm'
                            className='h-auto gap-1 p-0 text-xs'
                            onClick={() => handleReissue(device)}
                            disabled={reissueLink.isPending}
                            data-testid={`settings-crew-kiosk-new-link-${device.id}`}
                          >
                            {reissueLink.isPending &&
                            reissueLink.variables?.deviceId === device.id ? (
                              <Loader2 className='h-3 w-3 animate-spin' />
                            ) : (
                              <QrCode className='h-3 w-3' />
                            )}
                            New link
                          </Button>
                        )}
                      </div>
                    </TableCell>
                    <TableCell data-label='Opens' className='px-4 py-3 max-md:px-0 max-md:py-0'>
                      {app ? (
                        <span className='text-sm'>{app.name}</span>
                      ) : device.return_to ? (
                        <span
                          className='block max-w-48 truncate font-mono text-muted-foreground text-xs'
                          title={device.return_to}
                        >
                          {device.return_to}
                        </span>
                      ) : (
                        <span className='text-muted-foreground text-xs'>Organization home</span>
                      )}
                    </TableCell>
                    {/* Editable in place: a store tunes this after the first
                        shift, and the tablet keeps its binding through it. A
                        revoked kiosk is not editable — the server 409s, because
                        that row records which tablet a shift was signed in on. */}
                    <TableCell
                      data-label='Signs out'
                      className='px-4 py-3 text-sm max-md:px-0 max-md:py-0'
                      data-testid={`settings-crew-kiosk-idle-${device.id}`}
                    >
                      <KioskIdleCell orgId={orgId} device={device} editable={state !== "revoked"} />
                    </TableCell>
                    <TableCell className='w-12 px-2 py-3 text-right max-md:w-auto max-md:px-0 max-md:py-0'>
                      {state !== "revoked" && (
                        <Button
                          variant='ghost'
                          size='icon'
                          className='h-8 w-8 text-muted-foreground hover:text-destructive'
                          onClick={() => setPendingRevoke(device)}
                          title='Revoke kiosk'
                          aria-label={`Revoke ${device.name}`}
                        >
                          <Ban className='h-4 w-4' />
                        </Button>
                      )}
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        </TableWrapper>
      )}

      <NewKioskDialog
        open={creating}
        onOpenChange={setCreating}
        orgId={orgId}
        orgSlug={orgSlug}
        apps={apps}
        onCreated={setCreated}
      />
      <EnrolLinkDialog device={created} onClose={() => setCreated(null)} />

      <AlertDialog
        open={pendingRevoke !== null}
        onOpenChange={(open) => !open && setPendingRevoke(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Revoke {pendingRevoke?.name}?</AlertDialogTitle>
            <AlertDialogDescription>
              Crew sign-in stops working on that tablet at once, and its enroll link with it. To
              bring it back, enroll it again as a new kiosk.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className='bg-destructive text-destructive-foreground hover:bg-destructive/90'
              onClick={(e) => {
                e.preventDefault();
                handleRevoke();
              }}
              disabled={revokeDevice.isPending}
              data-testid='settings-crew-revoke-confirm'
            >
              {revokeDevice.isPending && <Loader2 className='h-4 w-4 animate-spin' />}
              Revoke
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

/** The one line under the badge that says *when*. */
function stateDetail(state: KioskState, device: KioskDeviceRow): string {
  switch (state) {
    case "bound":
      return device.last_seen_at
        ? `Last seen ${timeAgo(device.last_seen_at)}`
        : `Bound ${timeAgo(device.bound_at ?? device.created_at)}`;
    case "waiting":
      return device.enrol_expires_at ? `Link expires ${timeAgo(device.enrol_expires_at)}` : "";
    case "expired":
      return device.enrol_expires_at
        ? `Link expired ${timeAgo(device.enrol_expires_at)}`
        : "No live link";
    case "revoked":
      return `Revoked ${timeAgo(device.revoked_at ?? device.created_at)}`;
  }
}
