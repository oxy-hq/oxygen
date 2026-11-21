import { BarChart3, ChevronsRight, LucideActivity, Zap } from "lucide-react";
import type React from "react";
import { useState } from "react";
import { Link, Outlet, useLocation } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup
} from "@/components/ui/shadcn/resizable";
import {
  SidebarContent,
  SidebarGroup,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem
} from "@/components/ui/shadcn/sidebar";
import ROUTES from "@/libs/utils/routes";
import { SidebarHeader } from "@/pages/ide/components/SidebarHeader";
import useCurrentOrg from "@/stores/useCurrentOrg";

const ObservabilitySidebar: React.FC<{
  setSidebarOpen: (open: boolean) => void;
}> = ({ setSidebarOpen }) => {
  const location = useLocation();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";

  return (
    <div className='flex h-full flex-col overflow-hidden bg-sidebar-background'>
      <SidebarHeader title='Observability' onCollapse={() => setSidebarOpen(false)} />
      <SidebarContent className='h-full flex-1 overflow-y-auto'>
        <SidebarGroup className='px-1 pt-2'>
          <SidebarMenu>
            <SidebarMenuItem>
              <SidebarMenuButton
                asChild
                isActive={
                  location.pathname === ROUTES.ORG(orgSlug).WORKSPACE().IDE.OBSERVABILITY.TRACES
                }
              >
                <Link to={ROUTES.ORG(orgSlug).WORKSPACE().IDE.OBSERVABILITY.TRACES}>
                  <Zap className='h-4 w-4' />
                  <span>Traces</span>
                </Link>
              </SidebarMenuButton>
            </SidebarMenuItem>
            <SidebarMenuItem>
              <SidebarMenuButton
                asChild
                isActive={
                  location.pathname === ROUTES.ORG(orgSlug).WORKSPACE().IDE.OBSERVABILITY.METRICS
                }
              >
                <Link to={ROUTES.ORG(orgSlug).WORKSPACE().IDE.OBSERVABILITY.METRICS}>
                  <BarChart3 className='h-4 w-4' />
                  <span>Metrics</span>
                </Link>
              </SidebarMenuButton>
            </SidebarMenuItem>
            <SidebarMenuItem>
              <SidebarMenuButton
                asChild
                isActive={
                  location.pathname ===
                  ROUTES.ORG(orgSlug).WORKSPACE().IDE.OBSERVABILITY.EXECUTION_ANALYTICS
                }
              >
                <Link to={ROUTES.ORG(orgSlug).WORKSPACE().IDE.OBSERVABILITY.EXECUTION_ANALYTICS}>
                  <LucideActivity className='h-4 w-4' />
                  <span>Execution Analytics</span>
                </Link>
              </SidebarMenuButton>
            </SidebarMenuItem>
          </SidebarMenu>
        </SidebarGroup>
      </SidebarContent>
    </div>
  );
};

const ObservabilityLayout: React.FC = () => {
  const [sidebarOpen, setSidebarOpen] = useState(true);

  return (
    <ResizablePanelGroup direction='horizontal' className='flex-1'>
      {sidebarOpen ? (
        <>
          <ResizablePanel defaultSize={20} minSize={10} className='min-w-[200px]'>
            <ObservabilitySidebar setSidebarOpen={setSidebarOpen} />
          </ResizablePanel>
          <ResizableHandle withHandle />
        </>
      ) : (
        <div className='flex items-start border-r bg-sidebar-background px-1 py-2'>
          <Button
            variant='ghost'
            size='icon'
            onClick={() => setSidebarOpen(true)}
            tooltip={{ content: "Expand Sidebar", side: "right" }}
            className='h-8 w-8'
          >
            <ChevronsRight className='h-4 w-4' />
          </Button>
        </div>
      )}
      <ResizablePanel defaultSize={sidebarOpen ? 80 : 100} minSize={20}>
        <Outlet />
      </ResizablePanel>
    </ResizablePanelGroup>
  );
};

export default ObservabilityLayout;
