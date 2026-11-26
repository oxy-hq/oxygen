import { AlertCircle } from "lucide-react";
import type React from "react";
import { useMemo, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/shadcn/alert";
import { Button } from "@/components/ui/shadcn/button";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { Spinner } from "@/components/ui/shadcn/spinner";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import useUnifiImport from "@/hooks/api/cameras/useUnifiImport";
import type { UnifiImportResult, UnifiPreviewResult } from "@/services/api";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

type Props = {
  /**
   * UniFi API key. Undefined when the dialog is in `scan-stored` mode —
   * the backend then uses the stored workspace credential for both the
   * preview that produced `data` and the import that follows.
   */
  apiKey?: string;
  data: UnifiPreviewResult;
  onBack: () => void;
  onImported: (result: UnifiImportResult) => void;
};

const PreviewStep: React.FC<Props> = ({ apiKey, data, onBack, onImported }) => {
  const { workspace } = useCurrentWorkspace();
  const importMut = useUnifiImport(workspace?.id);

  // Default: every site selected. Operators that want to bring in
  // their whole UniFi account hit Import without touching anything.
  const [selected, setSelected] = useState<Set<string>>(
    () => new Set(data.sites.map((s) => s.unifi_console_id))
  );

  const allSelected = useMemo(
    () => data.sites.length > 0 && selected.size === data.sites.length,
    [data.sites.length, selected.size]
  );
  const someSelected = selected.size > 0 && !allSelected;

  const toggleSite = (id: string, next: boolean) => {
    setSelected((prev) => {
      const out = new Set(prev);
      if (next) out.add(id);
      else out.delete(id);
      return out;
    });
  };

  const toggleAll = (next: boolean) => {
    setSelected(next ? new Set(data.sites.map((s) => s.unifi_console_id)) : new Set());
  };

  const handleImport = async () => {
    // Empty selection is guarded by the button being disabled, but
    // belt-and-braces against keyboard activation.
    if (selected.size === 0) return;
    const result = await importMut.mutateAsync({
      apiKey,
      consoleIds: Array.from(selected)
    });
    onImported(result);
  };

  return (
    <div className='flex min-w-0 flex-col gap-4'>
      <div className='text-muted-foreground text-sm'>
        Found <span className='font-medium text-foreground'>{data.total_sites}</span> sites with{" "}
        <span className='font-medium text-foreground'>{data.total_cameras}</span> cameras across
        your UniFi account.
      </div>

      <div className='max-h-72 min-w-0 overflow-auto border'>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className='w-10'>
                <Checkbox
                  checked={allSelected ? true : someSelected ? "indeterminate" : false}
                  onCheckedChange={(v) => toggleAll(v === true)}
                  aria-label='Select all sites'
                />
              </TableHead>
              <TableHead>Site</TableHead>
              <TableHead>Hardware</TableHead>
              <TableHead>IP</TableHead>
              <TableHead className='text-right'>Cameras</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {data.sites.length === 0 ? (
              <TableRow>
                <TableCell colSpan={5} className='text-center text-muted-foreground'>
                  No sites found in this UniFi account.
                </TableCell>
              </TableRow>
            ) : (
              data.sites.map((s) => {
                const checked = selected.has(s.unifi_console_id);
                return (
                  <TableRow
                    key={s.unifi_console_id}
                    className='cursor-pointer'
                    onClick={() => toggleSite(s.unifi_console_id, !checked)}
                  >
                    <TableCell className='w-10' onClick={(e) => e.stopPropagation()}>
                      <Checkbox
                        checked={checked}
                        onCheckedChange={(v) => toggleSite(s.unifi_console_id, v === true)}
                        aria-label={`Select ${s.name}`}
                      />
                    </TableCell>
                    <TableCell className='font-medium'>{s.name}</TableCell>
                    <TableCell className='text-muted-foreground'>
                      {s.hardware_model ?? "—"}
                    </TableCell>
                    <TableCell className='text-muted-foreground'>{s.public_ip ?? "—"}</TableCell>
                    <TableCell className='text-right'>
                      {s.online_camera_count}/{s.camera_count}
                    </TableCell>
                  </TableRow>
                );
              })
            )}
          </TableBody>
        </Table>
      </div>

      {importMut.isError && (
        <Alert variant='destructive'>
          <AlertCircle />
          <AlertTitle>Import failed</AlertTitle>
          <AlertDescription>{(importMut.error as Error).message}</AlertDescription>
        </Alert>
      )}

      <div className='flex justify-between'>
        <Button type='button' variant='ghost' onClick={onBack} disabled={importMut.isPending}>
          Back
        </Button>
        <Button onClick={handleImport} disabled={importMut.isPending || selected.size === 0}>
          {importMut.isPending ? (
            <Spinner />
          ) : (
            `Import ${selected.size} site${selected.size === 1 ? "" : "s"}`
          )}
        </Button>
      </div>
    </div>
  );
};

export default PreviewStep;
