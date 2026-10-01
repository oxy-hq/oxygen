import useCurrentUser from "@/hooks/api/users/useCurrentUser";

/**
 * Whether this person may see workspace previews at all — `undefined` while
 * the profile is still loading, so a `?preview=` link can wait for the answer
 * instead of rendering live data first and switching to the preview after.
 *
 * Previews are an Oxy-staff tool, never the customer's: the Previews settings
 * tab, the IDE's "Open preview" and the "New preview" control are hidden from
 * everyone else, and a `?preview=` link a customer opens does nothing.
 *
 * Not a new flag. It is the platform-staff standing the rest of the web-app
 * already gates staff-only UI on — `is_owner || is_app_admin`, exactly what
 * decides the rail's Admin entry (`WorkspaceShell`) and `useActingSession`'s
 * `isStaff` — named here so every preview surface reads the one answer. UX
 * only, like those: the server decides what a caller may actually do.
 *
 * Staff reach a tenant's workspace only through an assume-role session, and a
 * Global Owner is kept in the admin console *until* they hold one (see
 * `OwnerRedirect`) — so for the owner this answer matters while acting.
 */
export default function useCanUsePreviews(): boolean | undefined {
  const { data: user, isPending } = useCurrentUser();
  if (isPending) return undefined;
  return !!(user?.is_owner || user?.is_app_admin);
}
