import { useCallback, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup
} from "@/components/ui/shadcn/resizable";
import { Sheet, SheetContent, SheetHeader, SheetTitle } from "@/components/ui/shadcn/sheet";
import { useAdminApp } from "@/hooks/api/customApps/useCustomApps";
import { useMediaQuery } from "@/hooks/useMediaQuery";
import type { CustomApp } from "@/types/apps";
import { type ChannelView, DetailToolbar, type Device } from "./components/DetailToolbar";
import { DockControls, DossierBody, DossierHeader } from "./components/Dossier";
import { LivePreview } from "./components/LivePreview";
import { DOCK_STORAGE_KEY, type DockMode, dossierWindowPath, reviveDockMode } from "./dock";
import { defaultChannel, draftTarget } from "./draftTarget";
import { useAppViewState } from "./useAppViewState";
import { useDossierWindow } from "./useDossierWindow";
import { usePersistentState } from "./usePersistentState";

/**
 * The stage: a live app preview beside a scrolling "dossier" (status & manifest,
 * builds, functions, activity, settings) — everything about one app on one
 * surface, no sub-tabs.
 *
 * The dossier is dockable, DevTools-style, because a fixed side column spends
 * the operator's scarcest resource (horizontal space on a laptop) on the
 * content least able to use it — manifest JSON, bundle paths, build rows. So:
 * dock right, dock bottom for the full stage width, or pop out into a real
 * second window. The choice persists per operator.
 *
 * Below `lg` none of that applies — the dossier folds into an overlay `Sheet`
 * so the toolbar controls never get squeezed off the row.
 *
 * ## Where the state lives
 *
 * **Draft is the staging host.** The channel is view state and nothing else:
 * Published frames the app's URL, Draft frames `staging_url` from the app's detail
 * response (the registry row this component is handed leaves it absent). With no
 * staging host — local dev has no customer-apps zone — Draft is disabled and says
 * so; it never falls back to the production URL. An app never promoted still has
 * a live URL serving its only build, so Published stays enabled as **Live** and
 * says its writes are real (`draftTarget.ts` owns all of these decisions). The staff `oxy_preview_draft`
 * cookie this used to flip is retired.
 *
 * Device, channel and the preview's own location are **query params**, because
 * they name a place an operator sends a colleague: "Bookkeeping, draft channel,
 * on mobile, showing the vendor screen". Back and Forward then walk those
 * choices, and a reload lands on the same view.
 *
 * Dock mode and whether the dossier is pinned stay in `localStorage`. They are
 * how one operator likes to sit, not a place — a shared link that rearranged
 * the recipient's panels would be a bug rather than a feature. `appViewState.ts`
 * states that split once.
 *
 * The reload nonce stays local: it is an instruction, not a location. Encoding
 * it would mean a shared link forces a refetch, and Back would "un-reload".
 */
export const AppDetail = ({ app: listed }: { app: CustomApp }) => {
  // The detail response is the registry row plus `staging_url`. Until it lands
  // the stage renders from the row, and Draft reads "pending".
  const detailQuery = useAdminApp(listed.id);
  const detail = detailQuery.data ?? null;
  const app = detail ?? listed;
  const draft = draftTarget(detail, detailQuery.isError);

  // What "no ?channel" opens on is per app (`defaultChannel`): an unpromoted
  // app opens on its staging host, or Live when it has none. The default is
  // passed in rather than baked into the reader, so a bare URL means the right
  // thing per app instead of the same thing for every app.
  const channelDefault: ChannelView = defaultChannel(app, draft);
  const { view, patch: patchView } = useAppViewState(channelDefault);
  const { device, channel } = view;

  const setDevice = useCallback((next: Device) => patchView({ device: next }), [patchView]);
  const onPreviewPathChange = useCallback(
    (path: string | null) => patchView({ preview: path }, "replace"),
    [patchView]
  );
  const onFnChange = useCallback((name: string | null) => patchView({ fn: name }), [patchView]);

  const [nonce, setNonce] = useState(0);

  // Wide = docked panel; narrow = overlay Sheet. Two bits of state so the
  // docked panel and the drawer keep independent defaults (panel open by
  // default; drawer closed until asked for).
  const isWide = useMediaQuery("(min-width: 1024px)");
  const [dossierPinned, setDossierPinned] = useState(true);
  const [sheetOpen, setSheetOpen] = useState(false);
  const dossierShown = isWide ? dossierPinned : sheetOpen;
  const toggleDossier = () => (isWide ? setDossierPinned((o) => !o) : setSheetOpen((o) => !o));

  const [dock, setDock] = usePersistentState<DockMode>(DOCK_STORAGE_KEY, "right", reviveDockMode);
  // A persisted `window` must NOT auto-open on load: a popup with no user
  // gesture is blocked, which would both toast an error and clobber the saved
  // preference. So window mode only actually pops out once the operator picks it
  // (a real gesture, tracked here); a persisted `window` renders inline as a
  // right dock until then, with the stored choice left intact so one click on
  // the control re-opens it.
  const [windowActivated, setWindowActivated] = useState(false);
  const effectiveDock: DockMode = dock === "window" && !windowActivated ? "right" : dock;
  const poppedOut = isWide && dossierPinned && dock === "window" && windowActivated;
  const handleDockChange = useCallback(
    (next: DockMode) => {
      // Selecting `window` from the control IS the gesture that lets the popup
      // open; record it so the open effect is allowed to run this time.
      if (next === "window") setWindowActivated(true);
      setDock(next);
    },
    [setDock]
  );
  // Closing the popped-out window (or having a user-initiated open blocked) must
  // land somewhere visible, not on an invisible dossier the operator can't get
  // back — and reset the gesture so it doesn't try to reopen on its own.
  const fallBackToSideColumn = useCallback(() => {
    setWindowActivated(false);
    setDock("right");
  }, [setDock]);
  const focusDossierWindow = useDossierWindow({
    active: poppedOut,
    url: dossierWindowPath(app.org_slug, app.slug),
    name: "oxy-app-dossier",
    onDismiss: fallBackToSideColumn
  });

  // View state only: the URL records the choice and the frame follows it. A
  // remount (nonce) lands the other build even when the path is unchanged.
  const handleChannelChange = (next: ChannelView) => {
    if (next === channel) return;
    patchView({ channel: next, preview: null });
    setNonce((n) => n + 1);
  };

  const preview = (
    <div className='flex h-full min-h-0 flex-col'>
      <LivePreview
        app={app}
        device={device}
        channel={channel}
        draft={draft}
        nonce={nonce}
        path={view.preview}
        onPathChange={onPreviewPathChange}
      />
    </div>
  );

  const dossier = (
    <div className='flex h-full min-h-0 flex-col'>
      <DossierHeader
        dock={effectiveDock}
        onDockChange={handleDockChange}
        onClose={() => setDossierPinned(false)}
      />
      <DossierBody app={app} focusSection={view.section} fn={view.fn} onFnChange={onFnChange} />
    </div>
  );

  const isDockedPanel = isWide && dossierPinned && effectiveDock !== "window";
  const bottom = effectiveDock === "bottom";

  return (
    <div className='flex h-full min-h-0 flex-col bg-background'>
      <DetailToolbar
        app={app}
        tab='preview'
        device={device}
        channel={channel}
        draft={draft}
        showTabs={false}
        dossierOpen={dossierShown}
        onToggleDossier={toggleDossier}
        onTabChange={() => undefined}
        onDeviceChange={setDevice}
        onChannelChange={handleChannelChange}
        onReload={() => setNonce((n) => n + 1)}
      />

      <div className='min-h-0 flex-1'>
        {isDockedPanel ? (
          // Keyed by direction: react-resizable-panels sizes against a fixed
          // axis, so flipping horizontal↔vertical needs a fresh group.
          <ResizablePanelGroup
            key={effectiveDock}
            direction={bottom ? "vertical" : "horizontal"}
            className='h-full min-h-0'
          >
            <ResizablePanel defaultSize={bottom ? 55 : 58} minSize={bottom ? 20 : 32}>
              {preview}
            </ResizablePanel>
            <ResizableHandle withHandle />
            <ResizablePanel defaultSize={bottom ? 45 : 42} minSize={bottom ? 20 : 26}>
              {dossier}
            </ResizablePanel>
          </ResizablePanelGroup>
        ) : (
          preview
        )}
      </div>

      {/* Popped out: the preview owns the whole stage, and this strip keeps the
          dock switcher reachable — otherwise the only way back to a docked
          panel would be to close the window we just opened. */}
      {poppedOut && (
        <div className='flex h-9 shrink-0 items-center gap-2 border-t bg-background px-2'>
          <span className='ml-1 min-w-0 flex-1 truncate text-muted-foreground text-xs'>
            Details are open in a separate window.
          </span>
          <Button variant='ghost' size='sm' className='h-7' onClick={() => focusDossierWindow()}>
            Focus window
          </Button>
          <DockControls value={effectiveDock} onChange={handleDockChange} />
        </div>
      )}

      {!isWide && (
        <Sheet open={sheetOpen} onOpenChange={setSheetOpen}>
          <SheetContent side='right' className='flex w-full flex-col gap-0 p-0 sm:max-w-md'>
            <SheetHeader className='shrink-0 border-b px-4 py-3'>
              <SheetTitle className='text-xs'>Status &amp; details</SheetTitle>
            </SheetHeader>
            <DossierBody
              app={app}
              focusSection={view.section}
              fn={view.fn}
              onFnChange={onFnChange}
            />
          </SheetContent>
        </Sheet>
      )}
    </div>
  );
};
