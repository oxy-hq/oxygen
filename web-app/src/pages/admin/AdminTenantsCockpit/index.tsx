import { ChevronLeft } from "lucide-react";
import { useEffect } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { useAdminPartners } from "@/hooks/api/adminPartners";
import { cn } from "@/libs/shadcn/utils";
import AdminOrgDetail from "@/pages/admin/AdminOrgs/AdminOrgDetail";
import AdminUserDetail from "@/pages/admin/AdminUsers/AdminUserDetail";
import AdminWorkspaceDetail from "@/pages/admin/AdminWorkspaces/AdminWorkspaceDetail";
import PartnerPane from "./components/panes/PartnerPane";
import TenantHeader from "./components/TenantHeader";
import { pushRecent } from "./components/TenantHeader/components/TenantSwitcher";
import TenantMap from "./components/TenantMap";
import TenantRail from "./components/TenantRail";
import { type TenantType, useTenantSelection } from "./useTenantSelection";

/** What the phone-only back button returns to, per tenant type. */
const SWITCHER_LABEL: Record<TenantType, string> = {
  orgs: "organizations",
  partners: "partners",
  users: "users",
  // No list of workspaces exists: the way back from one goes to the organizations.
  workspaces: "organizations"
};

/**
 * Tenant management: a relationship-first surface for orgs, partners, and users.
 * A dense rail (with partner chips) beside an inline detail dossier, plus a Map
 * view of who-manages-whom. Workspaces live one level down, inside their org.
 * Every action is inline — operators never round-trip through pages.
 */
export default function AdminTenantsCockpit() {
  const {
    type,
    id,
    view,
    partnerFilter,
    setType,
    setId,
    backToList,
    setView,
    focus,
    setPartnerFilter
  } = useTenantSelection();

  const partnerFilterName = usePartnerName(partnerFilter);

  // Record every opened tenant, so the switcher's "Recent" list reflects where the
  // operator has actually been rather than only where they jumped from the switcher.
  useEffect(() => {
    if (type === "orgs" && id) pushRecent(id);
  }, [type, id]);

  return (
    <div
      data-testid='admin-tenants-cockpit'
      className='flex h-[calc(100vh-3.5rem)] min-h-0 flex-col supports-[height:100dvh]:h-[calc(100dvh-3.5rem)]'
    >
      <TenantHeader
        type={type}
        id={id}
        onTypeChange={setType}
        onSelect={setId}
        view={view}
        onViewChange={setView}
        onCreatedOrg={setId}
      />

      {view === "map" ? (
        <main className='min-h-0 flex-1 overflow-auto'>
          <TenantMap onFocus={focus} />
        </main>
      ) : (
        // A phone has no room for the rail beside the dossier: it shows the rail until a
        // tenant is picked, then the dossier with a way back. Desktop keeps both side by side.
        <div className='flex min-h-0 flex-1'>
          <aside
            className={cn(
              "relative flex w-80 shrink-0 flex-col border-r max-md:w-full max-md:border-r-0",
              id && "max-md:hidden"
            )}
          >
            <TenantRail
              type={type}
              selectedId={id}
              onSelect={setId}
              partnerFilter={partnerFilter}
              partnerFilterName={partnerFilterName}
              onClearPartnerFilter={() => setPartnerFilter(null)}
              onPartnerChip={setPartnerFilter}
            />
          </aside>

          <main className={cn("min-w-0 flex-1 overflow-auto", !id && "max-md:hidden")}>
            {id && (
              <div className='border-b bg-background px-2 py-1 md:hidden'>
                <Button
                  variant='ghost'
                  size='sm'
                  className='h-8 gap-1 text-xs'
                  onClick={backToList}
                  data-testid='admin-tenants-back-to-list'
                >
                  <ChevronLeft className='size-3.5' aria-hidden />
                  All {SWITCHER_LABEL[type]}
                </Button>
              </div>
            )}
            <DetailStage type={type} id={id} onClose={() => setId(null)} />
          </main>
        </div>
      )}
    </div>
  );
}

function DetailStage({
  type,
  id,
  onClose
}: {
  type: TenantType;
  id: string | null;
  onClose: () => void;
}) {
  if (!id) return <EmptyStage />;
  // Orgs / users / workspaces reuse the full standalone admin detail surfaces (the
  // Tenant-360 views) inline via an `embedded` flag — one detail implementation,
  // not a second cockpit-only copy. Cross-links inside them navigate through the
  // URL, so focusing a related tenant lands back here.
  if (type === "orgs") return <AdminOrgDetail orgId={id} embedded />;
  if (type === "users") return <AdminUserDetail userId={id} embedded />;
  if (type === "workspaces") return <AdminWorkspaceDetail workspaceId={id} embedded />;
  // Partners have no standalone route yet, so they keep the inline pane.
  return <PartnerPane partnerId={id} onClose={onClose} />;
}

function usePartnerName(partnerId: string | null): string | undefined {
  const { data } = useAdminPartners();
  if (!partnerId) return undefined;
  return data?.find((p) => p.org_id === partnerId)?.name;
}

function EmptyStage() {
  return (
    <div className='flex h-full items-center justify-center p-8 text-center'>
      <p className='max-w-sm text-muted-foreground text-xs'>
        Select an entity to inspect and manage it — roles, transfers, and partnerships are all
        editable inline here.
      </p>
    </div>
  );
}
