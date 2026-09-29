import { CheckCircle2 } from "lucide-react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import ROUTES from "@/libs/utils/routes";

/** After "Leave kiosk mode": the kiosk is revoked and its cookie cleared. */
export default function LeftKiosk() {
  return (
    <div className='flex flex-col gap-4' data-testid='kiosk-manage-left'>
      <CheckCircle2 className='h-8 w-8 text-primary' aria-hidden='true' />
      <div className='flex flex-col gap-1'>
        <h1 className='font-semibold text-xl'>This browser is back to normal</h1>
        <p className='text-muted-foreground text-sm'>
          It is no longer a store tablet: the login page asks for an account again, in every Oxygen
          app. The kiosk shows as Revoked in Settings → Crew — to use a tablet here again, create a
          new kiosk there.
        </p>
      </div>
      <Button asChild className='self-start'>
        <Link to={ROUTES.ROOT} data-testid='kiosk-manage-left-home'>
          Open Oxygen
        </Link>
      </Button>
    </div>
  );
}
