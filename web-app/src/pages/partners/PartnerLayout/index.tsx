import { useMemo, useState } from "react";
import { Outlet, useLocation } from "react-router-dom";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { SidebarInset, SidebarProvider } from "@/components/ui/shadcn/sidebar";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useMyPartners } from "@/hooks/api/partners";
import ROUTES from "@/libs/utils/routes";
import { PartnerConsoleContext } from "../context";
import { PartnerSidebar } from "./components/PartnerSidebar";
import { PartnerTopbar } from "./components/PartnerTopbar";

const TITLES: [string, string][] = [
  [ROUTES.PARTNERS.APPS, "Custom apps"],
  [ROUTES.PARTNERS.TEAM, "Team"],
  [ROUTES.PARTNERS.ACTIVITY, "Activity"],
  [ROUTES.PARTNERS.ROOT, "Clients"]
];

/**
 * The partner console shell — deliberately the same shell as `/admin`.
 *
 * Both are operations surfaces for someone administering organizations they don't
 * personally own, so they get the same sidebar, the same thin topbar, the same
 * table treatment. What differs between them is *reach*, not visual language; a
 * partner should feel like they're using the same product as Oxy staff, with less
 * of it available.
 */
export default function PartnerLayout() {
  const { data: partners, isLoading, error } = useMyPartners();
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const location = useLocation();

  const active = useMemo(
    () => partners?.find((p) => p.partner_id === selectedId) ?? partners?.[0],
    [partners, selectedId]
  );

  const title = TITLES.find(([path]) => location.pathname.startsWith(path))?.[1] ?? "Clients";

  if (isLoading) {
    return (
      <div className='flex h-full w-full items-center justify-center'>
        <Spinner className='size-6' />
      </div>
    );
  }

  // No partner role anywhere — an empty console would be a lie about what they
  // hold, so say so plainly instead.
  if (error || !active) {
    return (
      <div className='flex h-full flex-col items-center justify-center gap-1 p-6 text-center'>
        <h1 className='font-semibold text-lg'>Partner console</h1>
        <p className='text-muted-foreground text-sm'>
          {error
            ? "Failed to load your partners."
            : "You don't hold a partner role at any organization."}
        </p>
      </div>
    );
  }

  return (
    <PartnerConsoleContext.Provider
      value={{ partners: partners ?? [], active, select: setSelectedId }}
    >
      {/* `DESIGN_REFERENCE` for the reason the comment above gives: what differs from
          `/admin` is reach, not visual language. It is also load-bearing, not cosmetic —
          this console is built from the admin kit (`AdminPage`, `ADMIN_TONE`,
          `AdminTable`, `RoleBadge`), and those now speak in the reference's roles
          (`bg-field`, `bg-dot-*`, `text-subtle-foreground`), which only have values
          inside this subtree. Without the class the kit would render uncoloured here. */}
      <SidebarProvider className={DESIGN_REFERENCE}>
        <PartnerSidebar />
        <SidebarInset>
          <PartnerTopbar title={title} />
          <div className='min-h-0 flex-1 overflow-auto'>
            <Outlet />
          </div>
        </SidebarInset>
      </SidebarProvider>
    </PartnerConsoleContext.Provider>
  );
}
