import { Navigate, Outlet, useLocation } from "react-router-dom";
import { SidebarInset, SidebarProvider } from "@/components/ui/shadcn/sidebar";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useActingSession } from "@/hooks/api/adminAssume/useActingSession";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import {
  adminPageGroup,
  adminPageTitle,
  canReachAdminRoute,
  firstReachableAdminRoute
} from "./adminNav";
import { AdminSidebar } from "./components/AdminSidebar";
import { AdminTopbar } from "./components/AdminTopbar";

export default function AdminLayout() {
  const location = useLocation();
  const { data: user, isPending } = useCurrentUser();
  const title = adminPageTitle(location.pathname, location.search);
  const group = adminPageGroup(location.pathname, location.search);

  const acting = useActingSession();

  if (isPending) {
    return (
      <div className='flex h-full w-full items-center justify-center'>
        <Spinner className='size-6' />
      </div>
    );
  }

  const isOwner = user?.is_owner ?? false;
  const isAppAdmin = user?.is_app_admin ?? false;
  const capabilities = user?.platform_capabilities ?? [];

  // No admin role at all — go home. Avoids a "Failed to load" stuck state
  // for users who land on `/admin/apps` via a stale bookmark.
  if (!isOwner && !isAppAdmin) {
    return <Navigate to='/' replace />;
  }

  // Staff member on a route their standing does not reach — bounce to the first surface
  // their standing DOES reach, rather than paint a page of 403s.
  //
  // Asks the SAME map the sidebar filters on. This used to be a separate hardcoded list
  // of path prefixes, written when "not owner" meant "app admin" and there were no
  // capabilities; every capability added to a nav item afterwards made that item appear
  // and then bounce. `Staff access` is the one that caught it — visible in the rail,
  // redirecting on click, for every Global Admin.
  if (!canReachAdminRoute(location.pathname, { isOwner, capabilities })) {
    // Not a fixed route: Custom apps is itself gated on `manage_apps`, so a future
    // preset without it would be redirected to a page the guard bounces it off again.
    return <Navigate to={firstReachableAdminRoute({ isOwner, capabilities })} replace />;
  }

  // You cannot be in here while acting as a tenant. The server already refuses
  // the whole staff surface (assume::block_admin_while_acting), so rendering the
  // admin shell would just paint a page of 403s — but the deeper reason is that
  // "acting as" only means something if it changes where you are and what you can
  // do. Sitting in the admin panel with a banner on top changed neither.
  //
  // The banner's Stop button ends the session and brings you straight back.
  if (acting.isActing && acting.landing) {
    return <Navigate to={acting.landing} replace />;
  }

  return (
    <SidebarProvider>
      <AdminSidebar />
      <SidebarInset>
        <AdminTopbar title={title} group={group} />
        <div className='min-h-0 flex-1 overflow-auto'>
          <Outlet />
        </div>
      </SidebarInset>
    </SidebarProvider>
  );
}
