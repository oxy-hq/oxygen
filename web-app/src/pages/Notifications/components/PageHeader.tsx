import { ArrowLeft, Settings } from "lucide-react";
import { Link, useLocation, useNavigate } from "react-router-dom";
import { SETTINGS_PARAM } from "@/components/settings/settingsRoute";
import ROUTES from "@/libs/utils/routes";

/** Settings → Notifications, opened in whichever workspace `/` lands in. */
const SETTINGS_LINK = `${ROUTES.ROOT}?${SETTINGS_PARAM}=account.notifications`;

const ICON_BUTTON =
  "flex size-8 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-sidebar-accent hover:text-foreground";

/**
 * The page's own top bar — it lives outside any workspace, so it has no rail.
 * Back returns where the person came from (or home, on a fresh tab); the gear
 * opens notification settings.
 */
export function PageHeader() {
  const navigate = useNavigate();
  const location = useLocation();
  // "default" is the first entry of this tab's history: nothing to go back to.
  const back = () => (location.key === "default" ? navigate(ROUTES.ROOT) : navigate(-1));

  return (
    <header className='flex h-12 shrink-0 items-center gap-2 border-b bg-sidebar-background px-2'>
      <button
        type='button'
        onClick={back}
        aria-label='Back'
        data-testid='notifications-page-back'
        className={ICON_BUTTON}
      >
        <ArrowLeft className='size-4' />
      </button>
      <h1 className='min-w-0 flex-1 truncate font-medium text-sm'>Notifications</h1>
      <Link
        to={SETTINGS_LINK}
        aria-label='Notification settings'
        data-testid='notifications-page-settings'
        className={ICON_BUTTON}
      >
        <Settings className='size-4' />
      </Link>
    </header>
  );
}
