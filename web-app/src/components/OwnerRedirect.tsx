import { Navigate, Outlet } from "react-router-dom";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useActingSession } from "@/hooks/api/adminAssume/useActingSession";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import ROUTES from "@/libs/utils/routes";

/**
 * Wraps non-admin auth-gated routes and bounces OXY_OWNER users back to the
 * admin shell. The login callbacks already pick the admin queue as the
 * destination via `handlePostLoginOrgs`, so this only catches manual
 * navigation (typing a URL, browser back, or stale links). The server-side
 * `oxy_owner_guard` middleware remains the authoritative gate for admin
 * endpoints — this guard is UX-only.
 *
 * **A live assume-role session lifts it.** Acting as a tenant is the one
 * sanctioned way for staff into a tenant's product: the server grants the reach
 * only through that session, `AdminLayout` sends an acting operator *to* the
 * tenant (`useActingSession().landing`), and staff-only tools that live in the
 * product — workspace previews — are used from there. Bouncing an acting owner
 * back to the queue made that a loop (admin → tenant → admin) and left the
 * Global Owner the one staff member who could never reach a preview. Without a
 * session nothing changes: the owner still lands in the admin console.
 */
export default function OwnerRedirect() {
  const { data: user, isPending } = useCurrentUser();
  const acting = useActingSession();
  const isOwner = !!user?.is_owner;

  // Wait for the session list too, but only for an owner — deciding before it
  // answers would bounce an acting owner on the first render.
  if (isPending || (isOwner && acting.isPending)) {
    return (
      <div className='flex h-full w-full items-center justify-center'>
        <Spinner className='size-6' />
      </div>
    );
  }

  if (isOwner && !acting.isActing) {
    return <Navigate to={ROUTES.ADMIN.BILLING_QUEUE} replace />;
  }

  return <Outlet />;
}
