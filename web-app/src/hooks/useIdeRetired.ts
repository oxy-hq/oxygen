import { useOptionalAuth } from "@/contexts/AuthContext";

/**
 * Whether this deployment has retired the in-product IDE: the server's
 * `ide_retired` rollout flag, from `GET /auth/config`. On, the IDE's Files,
 * Database client, git header and chat's Build mode are hidden and the
 * repository panel is shown where Files was; the server refuses those routes
 * (410 `ide_retired`) whatever the client shows. Off, the IDE is as it was.
 *
 * Outside an `AuthProvider` it reads as off.
 */
export default function useIdeRetired(): boolean {
  const auth = useOptionalAuth();
  return auth?.authConfig.ide_retired === true;
}
