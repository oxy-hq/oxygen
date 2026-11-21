import { Loader2, Plus } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import type { useProvisionOltp, useSetOltpVisibility } from "@/hooks/api/oltp/useAdminOltp";
import { AdminSectionLabel } from "@/pages/admin/components/AdminSectionLabel";
import { OltpSchemaStrip } from "@/pages/admin/components/OltpSchemaStrip";
import type { OltpConnectionInfo } from "@/services/api/oltp";

/**
 * The writers inside this database, and the box that adds another.
 *
 * **The strip replaced a four-column table.** Schema / Kind / Role / Analytics
 * spent a full-width table on two rows, and its widest column held a role name
 * (`app_bookings_rw_b4473a69`) an operator never needs to read here — the
 * Connect panel beside it hands out the credential, and `oxyc oltp audit` is
 * where you go to check role authority. What is left is what the strip already
 * shows: which schemas exist, and whether analytics can read each one. Clicking
 * a chip toggles the thing it displays, so the control and its state are the
 * same object rather than a badge in one column and a button in another.
 */
export const OltpSchemasTable = ({
  data,
  visibility,
  provision
}: {
  data: OltpConnectionInfo;
  visibility: ReturnType<typeof useSetOltpVisibility>;
  provision: ReturnType<typeof useProvisionOltp>;
}) => {
  const [addWriter, setAddWriter] = useState("");
  const pendingWriter = visibility.isPending ? visibility.variables?.writer : undefined;

  return (
    <div className='flex flex-col gap-2' data-testid='admin-org-oltp-schemas'>
      <AdminSectionLabel trailing={String(data.schemas.length)}>Schemas</AdminSectionLabel>

      {data.schemas.length === 0 ? (
        <p className='text-muted-foreground text-xs'>
          No writers yet. Add one below as <code className='font-mono'>app:&lt;slug&gt;</code> or{" "}
          <code className='font-mono'>pipeline:&lt;source&gt;</code>.
        </p>
      ) : (
        <>
          <OltpSchemaStrip
            schemas={data.schemas}
            testIdPrefix='admin-org-oltp-schema'
            // The strip stores no writer ref, so map back through the row it
            // came from — `app:bookings`, not the schema name `app_bookings`.
            pendingSchema={
              data.schemas.find((s) => `${s.kind}:${s.writer_name}` === pendingWriter)?.schema
            }
            onToggle={(s) => {
              const row = data.schemas.find((x) => x.schema === s.schema);
              if (!row) return;
              visibility.mutate({
                writer: `${row.kind}:${row.writer_name}`,
                visible: !row.analytics_visible
              });
            }}
          />
          <p className='text-muted-foreground text-xs'>
            Filled means the read-only analyst can read it. Click to change — <code>app_*</code> is
            hidden by default because live app state may be regulated.
          </p>
        </>
      )}

      <div className='flex items-center gap-1.5'>
        <Input
          className='h-7 text-xs'
          placeholder='app:bookings or pipeline:toast'
          value={addWriter}
          onChange={(e) => setAddWriter(e.target.value)}
          data-testid='admin-org-oltp-add-writer'
        />
        <Button
          size='sm'
          variant='outline'
          className='h-7 shrink-0 px-2 text-xs'
          disabled={!addWriter.trim() || provision.isPending}
          onClick={() => {
            provision.mutate([addWriter.trim()]);
            setAddWriter("");
          }}
          data-testid='admin-org-oltp-add-writer-submit'
        >
          {provision.isPending ? (
            <Loader2 className='size-3 animate-spin' />
          ) : (
            <Plus className='size-3' />
          )}
          Add
        </Button>
      </div>
    </div>
  );
};
