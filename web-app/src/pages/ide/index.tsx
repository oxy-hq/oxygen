import { Monitor } from "lucide-react";
import { useEffect, useState } from "react";
import { Outlet, useNavigate } from "react-router-dom";
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

const Ide = () => (
  <>
    <div className='flex h-full flex-1 flex-col overflow-hidden'>
      <Header />
      <div className='flex flex-1 overflow-hidden'>
        <Sidebar />
        <Outlet />
      </div>
    </div>
    <MobileIdeWarning />
  </>
);

export default Ide;
