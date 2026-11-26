import { ChevronsLeft } from "lucide-react";
import type React from "react";
import { Button } from "@/components/ui/shadcn/button";
import { SidebarGroupLabel } from "@/components/ui/shadcn/sidebar";

interface SidebarHeaderProps {
  title: React.ReactNode;
  onCollapse: () => void;
  actions?: React.ReactNode;
}

/**
 * A shared header component for IDE sidebars (Database, Files, Observability, Settings).
 * Provides consistent styling for the title, optional action buttons, and collapse button.
 */
export const SidebarHeader: React.FC<SidebarHeaderProps> = ({ title, onCollapse, actions }) => {
  return (
    <SidebarGroupLabel className='flex h-auto min-h-[41px] items-center justify-between border-border-subtle border-b px-2 py-1'>
      <span className='font-semibold text-sm'>{title}</span>
      <div className='flex items-center gap-0.5'>
        {actions}
        <Button
          className='md:hidden'
          variant='ghost'
          size='sm'
          onClick={onCollapse}
          tooltip='Collapse Sidebar'
        >
          <ChevronsLeft className='h-4 w-4' />
        </Button>
      </div>
    </SidebarGroupLabel>
  );
};
