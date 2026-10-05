import { Monitor } from "lucide-react";
import { createContext, useContext, useEffect, useState } from "react";
import { Outlet, useLocation, useNavigate, useResolvedPath } from "react-router-dom";
import ProjectStatus from "@/components/ProjectStatus";
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
import useSidebar from "@/components/ui/shadcn/sidebar-context";
import Header from "./Header";
import Sidebar from "./Sidebar";

type IdeScope = { insideIDE: boolean };
const INSIDE_IDE: IdeScope = { insideIDE: true };
const OUTSIDE_IDE: IdeScope = { insideIDE: false };

const IDEContext = createContext<IdeScope>(OUTSIDE_IDE);
export const useIDE = () => {
  return useContext(IDEContext);
};

/**
 * Sections mounted under `/ide` that are not the IDE, by their first path
 * segment (the routes are in `App.tsx`).
 *
 * What they show — schedules and runs, traces, the camera fleet — exists once
 * per workspace, not once per branch, so the branch selected in the header
 * means nothing to them. Sending it anyway is not harmless: the server routes
 * every request carrying `?branch=` to the one node that owns the workspace
 * files (`role_middleware::escalate_for_branch`), which pins a request any
 * replica could serve to that node and fails it whenever that node is down.
 *
 * Inside one of these, `useCurrentWorkspaceBranch().branchName` is "" — the
 * same as on every page outside `/ide`. The header and sidebar above them keep
 * the branch: they are the IDE's.
 *
 * A section missing from this list keeps sending the branch, which is the safe
 * way to be wrong. `branchHintRequests.test.tsx` pins the list.
 */
const BRANCH_INDEPENDENT_SECTIONS: ReadonlySet<string> = new Set([
  "coordinator",
  "observability",
  "edge"
]);

/** The scope of the section being shown: the first path segment under `/ide`. */
const useSectionScope = (): IdeScope => {
  const { pathname: idePath } = useResolvedPath("");
  const { pathname } = useLocation();
  const underIde = pathname.startsWith(idePath) ? pathname.slice(idePath.length) : "";
  const [section = ""] = underIde.split("/").filter(Boolean);
  return BRANCH_INDEPENDENT_SECTIONS.has(section) ? OUTSIDE_IDE : INSIDE_IDE;
};

const MobileIdeWarning = () => {
  const { isMobile } = useSidebar();
  const navigate = useNavigate();
  const [open, setOpenWarning] = useState(false);

  // Fire on every IDE entry so users see the warning each time they enter the
  // Developer Portal on a phone — sessionStorage-style "once per session"
  // suppression was confusing because the user lost track of the warning and
  // never saw it again after a single dismiss.
  useEffect(() => {
    if (!isMobile) return;
    setOpenWarning(true);
  }, [isMobile]);

  const handleContinue = () => {
    setOpenWarning(false);
  };

  const handleLeave = () => {
    setOpenWarning(false);
    navigate("/");
  };

  return (
    <AlertDialog open={open} onOpenChange={setOpenWarning}>
      <AlertDialogContent className='max-w-sm'>
        <AlertDialogHeader>
          <div className='mx-auto flex h-12 w-12 items-center justify-center rounded-full bg-muted text-foreground'>
            <Monitor className='h-6 w-6' />
          </div>
          <AlertDialogTitle className='text-center'>
            Developer Portal is built for desktop
          </AlertDialogTitle>
          <AlertDialogDescription className='text-center'>
            The file editor, SQL workbench, and diagram tooling here aren't optimized for small
            screens. For the best experience, open Oxy on a larger device.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter className='flex-col gap-2 sm:flex-col sm:gap-2 sm:space-x-0'>
          <AlertDialogAction onClick={handleContinue}>Continue anyway</AlertDialogAction>
          <AlertDialogCancel onClick={handleLeave} className='mt-0'>
            Go back home
          </AlertDialogCancel>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
};

const Ide = () => {
  const sectionScope = useSectionScope();
  return (
    <IDEContext.Provider value={INSIDE_IDE}>
      <div className='flex h-full flex-1 flex-col overflow-hidden'>
        <ProjectStatus />
        <Header />
        <div className='flex flex-1 overflow-hidden'>
          <Sidebar />
          <IDEContext.Provider value={sectionScope}>
            <Outlet />
          </IDEContext.Provider>
        </div>
      </div>
      <MobileIdeWarning />
    </IDEContext.Provider>
  );
};

export default Ide;
