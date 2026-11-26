import { Plus, Slash, Square, Trash2 } from "lucide-react";
import type React from "react";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useApproveCameraProposal from "@/hooks/api/cameras/useApproveCameraProposal";
import useCameraSnapshot from "@/hooks/api/cameras/useCameraSnapshot";
import usePatchCameraZones from "@/hooks/api/cameras/usePatchCameraZones";
import useRejectCameraProposal from "@/hooks/api/cameras/useRejectCameraProposal";
import type { CameraSummary } from "@/services/api";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

type Point = [number, number];
type Zone = { zone_id: string; polygon: Point[] };
type Line = { line_id: string; p1: Point; p2: Point };

type Props = {
  camera: CameraSummary | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

/**
 * UniFi-style geometry editor. Operator clicks "Add zone" to drop a
 * default rectangle covering the middle of the frame, then drags
 * corner handles to reshape. Dragging a midpoint handle (the small
 * dot between two corners) splits that edge — the midpoint becomes
 * a new corner and you keep dragging it. Lines work the same way:
 * "Add line" drops a default horizontal segment, endpoint handles
 * reposition, the midpoint handle translates the whole line.
 *
 * Why we moved off freehand click-to-draw: vertex-snap-to-close was
 * fiddly (no visible snap zone), users couldn't tell when the
 * polygon "took," and the dblclick close was racy with state
 * updates from the click queue. UniFi's design is the proven
 * answer — start from a sensible default, never let the user
 * draw an open polygon.
 *
 * Coordinate system: storage is in original-frame pixels. The
 * snapshot's natural size sets the SVG's `viewBox`, so points map
 * 1:1 regardless of how the dialog scales the visible area.
 *
 * Reader hot-swap (worker side) picks up the new geometry on the
 * next config poll (~30s) via `_cam_signature` change detection.
 */
const ZoneEditorDialog: React.FC<Props> = ({ camera, open, onOpenChange }) => {
  const { workspace } = useCurrentWorkspace();
  const { blobUrl, isLoading } = useCameraSnapshot(workspace?.id, camera?.id, 30_000, open);

  const [imgDims, setImgDims] = useState<{ w: number; h: number } | null>(null);
  const [zones, setZones] = useState<Zone[]>([]);
  const [lines, setLines] = useState<Line[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [drag, setDrag] = useState<DragState | null>(null);

  const svgRef = useRef<SVGSVGElement | null>(null);
  const patch = usePatchCameraZones(workspace?.id);
  const approveProposal = useApproveCameraProposal(workspace?.id);
  const rejectProposal = useRejectCameraProposal(workspace?.id);

  const hasProposal =
    camera != null && (camera.proposed_zones_json != null || camera.proposed_lines_json != null);

  // Load existing geometry whenever the dialog opens on a (new)
  // camera. Without the reset, Camera A's polygons would leak onto
  // Camera B's frame on a quick consecutive open.
  useEffect(() => {
    if (!open || !camera) return;
    setZones(((camera.zones_json as Zone[] | null) ?? []).map(cleanZone));
    setLines(((camera.lines_json as Line[] | null) ?? []).map(cleanLine));
    setSelectedId(null);
    setDrag(null);
  }, [open, camera]);

  // Capture the snapshot's natural dimensions for the viewBox so
  // coordinates stay in image-pixel space even when CSS scales
  // the SVG.
  useEffect(() => {
    if (!blobUrl) {
      setImgDims(null);
      return;
    }
    const img = new Image();
    img.onload = () => setImgDims({ w: img.naturalWidth, h: img.naturalHeight });
    img.src = blobUrl;
  }, [blobUrl]);

  const viewBox = imgDims ? `0 0 ${imgDims.w} ${imgDims.h}` : "0 0 1280 720";

  // Translate a client-space coordinate (e.g. from a PointerEvent)
  // into image-pixel space. Uses the SVG's bounding rect so handles
  // outside the SVG element still resolve correctly.
  const clientToImage = useCallback(
    (clientX: number, clientY: number): Point | null => {
      if (!svgRef.current || !imgDims) return null;
      const rect = svgRef.current.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) return null;
      const sx = imgDims.w / rect.width;
      const sy = imgDims.h / rect.height;
      return [Math.round((clientX - rect.left) * sx), Math.round((clientY - rect.top) * sy)];
    },
    [imgDims]
  );

  // ── Add / Delete ──────────────────────────────────────────────

  const addZone = () => {
    if (!imgDims) return;
    const { w, h } = imgDims;
    // Default rectangle covering the centered 60% of the frame —
    // matches UniFi's "give the operator something to grab on to"
    // affordance. Corners are clockwise from top-left.
    const z: Zone = {
      zone_id: `zone-${zones.length + 1}-${Date.now().toString(36)}`,
      polygon: [
        [Math.round(w * 0.2), Math.round(h * 0.2)],
        [Math.round(w * 0.8), Math.round(h * 0.2)],
        [Math.round(w * 0.8), Math.round(h * 0.8)],
        [Math.round(w * 0.2), Math.round(h * 0.8)]
      ]
    };
    setZones((zs) => [...zs, z]);
    setSelectedId(z.zone_id);
  };

  const addLine = () => {
    if (!imgDims) return;
    const { w, h } = imgDims;
    // Horizontal through the centerline, 60% wide. The direction
    // of "crossing" is left→right by convention; operators can
    // flip later via a future direction toggle.
    const l: Line = {
      line_id: `line-${lines.length + 1}-${Date.now().toString(36)}`,
      p1: [Math.round(w * 0.2), Math.round(h * 0.5)],
      p2: [Math.round(w * 0.8), Math.round(h * 0.5)]
    };
    setLines((ls) => [...ls, l]);
    setSelectedId(l.line_id);
  };

  const removeSelected = () => {
    if (!selectedId) return;
    setZones((zs) => zs.filter((z) => z.zone_id !== selectedId));
    setLines((ls) => ls.filter((l) => l.line_id !== selectedId));
    setSelectedId(null);
  };

  // ── Drag handling ─────────────────────────────────────────────
  //
  // We use pointer events + `setPointerCapture` so the cursor can
  // wander off the handle (or even out of the SVG) without losing
  // the drag. Coordinate translation happens in image-pixel space
  // via `clientToImage` so geometry stays correct even when the
  // dialog is resized during a drag.

  const startDrag = (e: React.PointerEvent<SVGElement>, initial: DragState) => {
    e.stopPropagation();
    const target = e.currentTarget;
    target.setPointerCapture(e.pointerId);
    // Midpoint-on-edge: insert a real vertex at the click position
    // first, then convert the drag into a vertex drag. This makes
    // "drag the midpoint to bend the edge" feel continuous — the
    // pointer doesn't snap when the new vertex appears.
    if (initial.kind === "zone-mid") {
      const insertAt = initial.afterIndex + 1;
      setZones((zs) =>
        zs.map((z) => {
          if (z.zone_id !== initial.zoneId) return z;
          const a = z.polygon[initial.afterIndex];
          const b = z.polygon[(initial.afterIndex + 1) % z.polygon.length];
          if (!a || !b) return z;
          const mid: Point = [Math.round((a[0] + b[0]) / 2), Math.round((a[1] + b[1]) / 2)];
          return {
            ...z,
            polygon: [...z.polygon.slice(0, insertAt), mid, ...z.polygon.slice(insertAt)]
          };
        })
      );
      setDrag({ kind: "zone-vertex", zoneId: initial.zoneId, index: insertAt });
      setSelectedId(initial.zoneId);
      return;
    }
    if (initial.kind === "line-body") {
      const start = clientToImage(e.clientX, e.clientY);
      if (!start) return;
      const target = lines.find((l) => l.line_id === initial.lineId);
      if (!target) return;
      setDrag({
        kind: "line-body",
        lineId: initial.lineId,
        startPoint: start,
        startP1: target.p1,
        startP2: target.p2
      });
      setSelectedId(initial.lineId);
      return;
    }
    setDrag(initial);
    if (initial.kind === "zone-vertex") setSelectedId(initial.zoneId);
    if (initial.kind === "line-end") setSelectedId(initial.lineId);
  };

  const onPointerMove = (e: React.PointerEvent<SVGSVGElement>) => {
    if (!drag) return;
    const p = clientToImage(e.clientX, e.clientY);
    if (!p) return;
    if (!imgDims) return;
    const clampedX = clamp(p[0], 0, imgDims.w);
    const clampedY = clamp(p[1], 0, imgDims.h);
    const np: Point = [clampedX, clampedY];

    if (drag.kind === "zone-vertex") {
      setZones((zs) =>
        zs.map((z) =>
          z.zone_id === drag.zoneId
            ? { ...z, polygon: z.polygon.map((pt, i) => (i === drag.index ? np : pt)) }
            : z
        )
      );
    } else if (drag.kind === "line-end") {
      setLines((ls) =>
        ls.map((l) =>
          l.line_id === drag.lineId
            ? drag.which === "p1"
              ? { ...l, p1: np }
              : { ...l, p2: np }
            : l
        )
      );
    } else if (drag.kind === "line-body") {
      const dx = np[0] - drag.startPoint[0];
      const dy = np[1] - drag.startPoint[1];
      setLines((ls) =>
        ls.map((l) =>
          l.line_id === drag.lineId
            ? {
                ...l,
                p1: [
                  clamp(drag.startP1[0] + dx, 0, imgDims.w),
                  clamp(drag.startP1[1] + dy, 0, imgDims.h)
                ],
                p2: [
                  clamp(drag.startP2[0] + dx, 0, imgDims.w),
                  clamp(drag.startP2[1] + dy, 0, imgDims.h)
                ]
              }
            : l
        )
      );
    }
  };

  const endDrag = () => setDrag(null);

  // ── Proposal preview / accept / reject ───────────────────────

  /** Replace the editor's working zones/lines with the
   *  proposal's payload so the operator can see what would be
   *  applied. Doesn't write anything — the operator still has
   *  to hit Save (writes via the regular zones PATCH) or
   *  Approve (promotes the proposal verbatim). */
  const onPreviewProposal = () => {
    if (!camera) return;
    setZones(((camera.proposed_zones_json as Zone[] | null) ?? []).map(cleanZone));
    setLines(((camera.proposed_lines_json as Line[] | null) ?? []).map(cleanLine));
    setSelectedId(null);
    toast.info("Previewing the proposal — Save to keep changes, Reset to revert.");
  };

  const onApproveProposal = async () => {
    if (!camera) return;
    try {
      await approveProposal.mutateAsync(camera.id);
      toast.success("Proposal applied as the live geometry.");
      onOpenChange(false);
    } catch (err) {
      const msg = err instanceof Error ? err.message : "approve failed";
      toast.error(`Could not approve proposal: ${msg}`);
    }
  };

  const onRejectProposal = async () => {
    if (!camera) return;
    try {
      await rejectProposal.mutateAsync(camera.id);
      toast.success("Proposal discarded.");
    } catch (err) {
      const msg = err instanceof Error ? err.message : "reject failed";
      toast.error(`Could not reject proposal: ${msg}`);
    }
  };

  // ── Save ──────────────────────────────────────────────────────

  const onSave = async () => {
    if (!camera) return;
    try {
      await patch.mutateAsync({
        cameraId: camera.id,
        zones_json: zones,
        lines_json: lines
      });
      toast.success(`Saved ${zones.length} zones / ${lines.length} lines`);
      onOpenChange(false);
    } catch (err) {
      const msg = err instanceof Error ? err.message : "save failed";
      toast.error(`Could not save triggers: ${msg}`);
    }
  };

  // ── Render ────────────────────────────────────────────────────

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={`${DESIGN_REFERENCE} max-w-5xl`}>
        <DialogHeader>
          <DialogTitle>Triggers · {camera?.name ?? "Camera"}</DialogTitle>
          <DialogDescription>
            Add a zone or a crossing line, then drag the handles to position it. Without geometry
            the worker falls back to a whole-frame check every minute.
          </DialogDescription>
        </DialogHeader>

        {hasProposal && camera && (
          <div className='flex flex-wrap items-center justify-between gap-3 border border-primary/40 bg-primary/5 p-3'>
            <div className='flex flex-col gap-0.5'>
              <span className='font-medium text-primary text-sm'>
                Proposed geometry pending review
              </span>
              <span className='text-muted-foreground text-xs'>
                {camera.proposed_at && `Proposed ${new Date(camera.proposed_at).toLocaleString()}.`}{" "}
                Preview to overlay on the snapshot, approve to make it live, or reject to discard.
              </span>
            </div>
            <div className='flex flex-wrap items-center gap-2'>
              <Button size='sm' variant='outline' onClick={onPreviewProposal}>
                Preview
              </Button>
              <Button
                size='sm'
                variant='ghost'
                onClick={onRejectProposal}
                disabled={rejectProposal.isPending}
              >
                Reject
              </Button>
              <Button size='sm' onClick={onApproveProposal} disabled={approveProposal.isPending}>
                Approve
              </Button>
            </div>
          </div>
        )}

        <div className='flex flex-wrap items-center gap-2'>
          <Button size='sm' variant='outline' onClick={addZone} disabled={!imgDims}>
            <Plus />
            <Square />
            Add zone
          </Button>
          <Button size='sm' variant='outline' onClick={addLine} disabled={!imgDims}>
            <Plus />
            <Slash />
            Add line
          </Button>
          <Button size='sm' variant='ghost' onClick={removeSelected} disabled={!selectedId}>
            <Trash2 />
            Remove selected
          </Button>
          <span className='ml-auto text-muted-foreground text-xs'>
            Drag corners to reshape · drag a midpoint to add a vertex
          </span>
        </div>

        <div className='grid gap-3 lg:grid-cols-[minmax(0,1fr)_240px]'>
          <div className='relative overflow-hidden border bg-muted/40'>
            {isLoading || !imgDims ? (
              <div className='flex aspect-video w-full items-center justify-center'>
                <Spinner />
              </div>
            ) : (
              <div className='relative'>
                {blobUrl && (
                  <img
                    src={blobUrl}
                    alt={`${camera?.name ?? "camera"} snapshot`}
                    className='w-full select-none'
                    draggable={false}
                  />
                )}
                {/* biome-ignore lint/a11y/useKeyWithClickEvents: drawing surface, pointer-driven by design */}
                <svg
                  ref={svgRef}
                  viewBox={viewBox}
                  className='absolute inset-0 h-full w-full'
                  onClick={() => setSelectedId(null)}
                  onPointerMove={onPointerMove}
                  onPointerUp={endDrag}
                  onPointerCancel={endDrag}
                  preserveAspectRatio='none'
                  role='application'
                  aria-label='Trigger geometry editor'
                >
                  <title>Trigger geometry editor</title>

                  {zones.map((z) => (
                    <ZoneShape
                      key={z.zone_id}
                      zone={z}
                      selected={selectedId === z.zone_id}
                      onSelect={() => setSelectedId(z.zone_id)}
                      onVertexDown={(idx, e) =>
                        startDrag(e, { kind: "zone-vertex", zoneId: z.zone_id, index: idx })
                      }
                      onMidDown={(after, e) =>
                        startDrag(e, { kind: "zone-mid", zoneId: z.zone_id, afterIndex: after })
                      }
                    />
                  ))}

                  {lines.map((l) => (
                    <LineShape
                      key={l.line_id}
                      line={l}
                      selected={selectedId === l.line_id}
                      onSelect={() => setSelectedId(l.line_id)}
                      onEndpointDown={(which, e) =>
                        startDrag(e, { kind: "line-end", lineId: l.line_id, which })
                      }
                      onBodyDown={(e) =>
                        startDrag(e, {
                          kind: "line-body",
                          lineId: l.line_id,
                          startPoint: [0, 0],
                          startP1: l.p1,
                          startP2: l.p2
                        })
                      }
                    />
                  ))}
                </svg>
              </div>
            )}
          </div>

          <aside className='flex flex-col gap-3 text-sm'>
            <ShapeList
              title='Zones'
              shapes={zones.map((z) => ({ id: z.zone_id, label: z.zone_id }))}
              selectedId={selectedId}
              onSelect={setSelectedId}
              onDelete={(id) => setZones((zs) => zs.filter((z) => z.zone_id !== id))}
              emptyHint='No zones. Worker falls back to whole-frame trigger.'
            />
            <ShapeList
              title='Lines'
              shapes={lines.map((l) => ({ id: l.line_id, label: l.line_id }))}
              selectedId={selectedId}
              onSelect={setSelectedId}
              onDelete={(id) => setLines((ls) => ls.filter((l) => l.line_id !== id))}
              emptyHint='No line-crossing counters.'
            />
            <p className='text-muted-foreground text-xs italic'>
              Click a shape to select. Edits apply on the next worker config poll (~30s).
            </p>
          </aside>
        </div>

        <DialogFooter>
          <Button variant='outline' onClick={() => onOpenChange(false)} disabled={patch.isPending}>
            Cancel
          </Button>
          <Button onClick={onSave} disabled={patch.isPending}>
            {patch.isPending ? "Saving…" : "Save triggers"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default ZoneEditorDialog;

// ── DragState ─────────────────────────────────────────────────────

type DragState =
  | { kind: "zone-vertex"; zoneId: string; index: number }
  | { kind: "zone-mid"; zoneId: string; afterIndex: number }
  | { kind: "line-end"; lineId: string; which: "p1" | "p2" }
  | {
      kind: "line-body";
      lineId: string;
      startPoint: Point;
      startP1: Point;
      startP2: Point;
    };

// ── Helpers ───────────────────────────────────────────────────────

const clamp = (v: number, lo: number, hi: number) => (v < lo ? lo : Math.min(v, hi));

/** Strip malformed entries from existing geometry. A persisted zone
 *  with < 3 points or a missing polygon array would crash render. */
const cleanZone = (z: Zone): Zone => ({
  zone_id: z.zone_id ?? `zone-${Date.now().toString(36)}`,
  polygon: Array.isArray(z.polygon)
    ? z.polygon.filter((p) => Array.isArray(p) && p.length === 2)
    : []
});

const cleanLine = (l: Line): Line => ({
  line_id: l.line_id ?? `line-${Date.now().toString(36)}`,
  p1: l.p1 ?? [0, 0],
  p2: l.p2 ?? [0, 0]
});

// ── Sub-components ────────────────────────────────────────────────

type ZoneShapeProps = {
  zone: Zone;
  selected: boolean;
  onSelect: () => void;
  onVertexDown: (index: number, e: React.PointerEvent<SVGCircleElement>) => void;
  onMidDown: (afterIndex: number, e: React.PointerEvent<SVGCircleElement>) => void;
};

const ZoneShape: React.FC<ZoneShapeProps> = ({
  zone,
  selected,
  onSelect,
  onVertexDown,
  onMidDown
}) => {
  if (zone.polygon.length < 3) return null;
  const pts = zone.polygon.map((p) => p.join(",")).join(" ");
  // Midpoints sit between consecutive vertices (last wraps to first).
  const midpoints = zone.polygon.map((p, i) => {
    const next = zone.polygon[(i + 1) % zone.polygon.length];
    return {
      after: i,
      cx: Math.round((p[0] + next[0]) / 2),
      cy: Math.round((p[1] + next[1]) / 2)
    };
  });
  return (
    <g>
      {/* biome-ignore lint/a11y/noStaticElementInteractions: select-on-click on a shape is the intent */}
      <polygon
        points={pts}
        className={
          selected
            ? "cursor-pointer fill-primary/40 stroke-[4] stroke-primary"
            : "cursor-pointer fill-primary/20 stroke-[3] stroke-primary"
        }
        onClick={(e) => {
          e.stopPropagation();
          onSelect();
        }}
      />
      {selected &&
        zone.polygon.map(([x, y], i) => (
          <circle
            // biome-ignore lint/suspicious/noArrayIndexKey: vertex index IS the stable identity here
            key={`v-${i}`}
            cx={x}
            cy={y}
            r={10}
            className='cursor-grab touch-none fill-primary stroke-2 stroke-background active:cursor-grabbing'
            onPointerDown={(e) => onVertexDown(i, e)}
          />
        ))}
      {selected &&
        midpoints.map((m) => (
          <circle
            key={`m-${m.after}`}
            cx={m.cx}
            cy={m.cy}
            r={6}
            className='cursor-grab touch-none fill-background stroke-2 stroke-primary active:cursor-grabbing'
            onPointerDown={(e) => onMidDown(m.after, e)}
          />
        ))}
    </g>
  );
};

type LineShapeProps = {
  line: Line;
  selected: boolean;
  onSelect: () => void;
  onEndpointDown: (which: "p1" | "p2", e: React.PointerEvent<SVGCircleElement>) => void;
  // Drag-start handler shared by the <line> body and the midpoint
  // <circle> grab handle. Typed against `SVGElement` so it fits
  // either prop slot — the handler only reads `e.pointerId` and
  // `clientX`/`clientY` (defined on both shapes).
  onBodyDown: (e: React.PointerEvent<SVGElement>) => void;
};

const LineShape: React.FC<LineShapeProps> = ({
  line,
  selected,
  onSelect,
  onEndpointDown,
  onBodyDown
}) => {
  const midX = Math.round((line.p1[0] + line.p2[0]) / 2);
  const midY = Math.round((line.p1[1] + line.p2[1]) / 2);
  return (
    <g>
      {/* biome-ignore lint/a11y/noStaticElementInteractions: select-on-click on a shape is the intent */}
      <line
        x1={line.p1[0]}
        y1={line.p1[1]}
        x2={line.p2[0]}
        y2={line.p2[1]}
        className={
          selected
            ? "cursor-move touch-none stroke-[6] stroke-primary"
            : "cursor-pointer stroke-[4] stroke-primary"
        }
        onClick={(e) => {
          e.stopPropagation();
          onSelect();
        }}
        onPointerDown={selected ? onBodyDown : undefined}
      />
      {selected && (
        <>
          <circle
            cx={line.p1[0]}
            cy={line.p1[1]}
            r={10}
            className='cursor-grab touch-none fill-primary stroke-2 stroke-background active:cursor-grabbing'
            onPointerDown={(e) => onEndpointDown("p1", e)}
          />
          <circle
            cx={line.p2[0]}
            cy={line.p2[1]}
            r={10}
            className='cursor-grab touch-none fill-primary stroke-2 stroke-background active:cursor-grabbing'
            onPointerDown={(e) => onEndpointDown("p2", e)}
          />
          <circle
            cx={midX}
            cy={midY}
            r={6}
            className='cursor-move touch-none fill-background stroke-2 stroke-primary'
            onPointerDown={onBodyDown}
          />
        </>
      )}
    </g>
  );
};

type ShapeListProps = {
  title: string;
  shapes: { id: string; label: string }[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onDelete: (id: string) => void;
  emptyHint: string;
};

const ShapeList: React.FC<ShapeListProps> = ({
  title,
  shapes,
  selectedId,
  onSelect,
  onDelete,
  emptyHint
}) => (
  <div>
    <div className='mb-2 flex items-center justify-between'>
      <span className='font-medium'>{title}</span>
      <Badge variant='secondary'>{shapes.length}</Badge>
    </div>
    {shapes.length === 0 ? (
      <p className='text-muted-foreground text-xs italic'>{emptyHint}</p>
    ) : (
      <ul className='space-y-1'>
        {shapes.map((s) => (
          <li
            key={s.id}
            className={
              s.id === selectedId
                ? "flex items-center justify-between border border-primary bg-primary/10 px-2 py-1 text-xs"
                : "flex items-center justify-between border border-border bg-card px-2 py-1 text-xs"
            }
          >
            <button
              type='button'
              className='truncate text-left hover:underline'
              onClick={() => onSelect(s.id)}
            >
              {s.label}
            </button>
            <Button size='icon' variant='ghost' className='h-6 w-6' onClick={() => onDelete(s.id)}>
              <Trash2 className='h-3 w-3' />
            </Button>
          </li>
        ))}
      </ul>
    )}
  </div>
);
