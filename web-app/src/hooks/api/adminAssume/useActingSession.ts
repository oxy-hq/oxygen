import { useCurrentAssume } from "@/hooks/api/adminAssume";
import useCurrentUser from "@/hooks/api/users/useCurrentUser";
import { peekAssumeReturnTo } from "@/libs/utils/assumeDestination";
import type { AssumeSession } from "@/types/adminAssume";

/**
 * Where an assumed session belongs.
 *
 * Acting as a **partner** lands you in the partner console; acting as a plain org
 * lands you in that org's product. Landing anywhere else — in particular, staying
 * in the admin panel — makes the mode pointless: you would be "acting as" someone
 * while looking at a screen they can't see.
 */
export function landingFor(session: AssumeSession): string {
  if (session.is_partner) return "/partners";
  return session.org_slug ? `/${session.org_slug}` : "/";
}

/**
 * The single answer to "am I currently acting as a tenant?".
 *
 * The server is the authority — it refuses the whole staff surface while a session
 * is live (`assume::block_admin_while_acting`) and synthesizes the tenant's reach
 * on the way in. This hook only keeps the UI from showing a door the server would
 * slam anyway.
 */
export function useActingSession(): {
  session: AssumeSession | undefined;
  isActing: boolean;
  landing: string | null;
  /** Whether this is Oxy staff — decides which console the mode closes. */
  isStaff: boolean;
  /** Where "stop acting" should return them: their own console. */
  home: string;
  /**
   * Where "stop acting" should return them *for this session*: the exact page
   * they left, when they entered from one (see `assumeDestination`), otherwise
   * `home`. Always a same-origin path.
   */
  returnTo: string;
  /**
   * The session list has not answered yet for someone who can hold one. A redirect
   * that depends on `isActing` waits on this; deciding early would bounce an acting
   * operator on the first render. Never true for anyone who can't act.
   */
  isPending: boolean;
} {
  const { data: user } = useCurrentUser();
  const isStaff = !!(user?.is_owner || user?.is_app_admin);
  const isPartner = (user?.partner_memberships?.length ?? 0) > 0;
  // Staff act as any org; a partner acts as an assigned client. Nobody else can
  // hold a session, so don't poll for them.
  const canAct = isStaff || isPartner;
  const { data: sessions, isPending } = useCurrentAssume(canAct);

  const session = canAct ? sessions?.[0] : undefined;
  // Staff came from admin; a partner came from their console. Returning someone
  // to a surface they can't reach would just 403 them at the door.
  const home = isStaff ? "/admin/tenants" : "/partners";
  return {
    session,
    isPending: canAct && isPending,
    isActing: !!session,
    landing: session ? landingFor(session) : null,
    isStaff,
    home,
    returnTo: peekAssumeReturnTo(session?.org_id) ?? home
  };
}
