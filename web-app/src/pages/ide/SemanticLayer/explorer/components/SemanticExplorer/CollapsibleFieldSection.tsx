import { ChevronDown, ChevronRight } from "lucide-react";
import { type ReactNode, useState } from "react";
import { SidebarMenuButton, SidebarMenuItem, SidebarMenuSub } from "@/components/ui/shadcn/sidebar";

interface CollapsibleFieldSectionProps {
  title: string;
  count: number;
  defaultOpen?: boolean;
  children: ReactNode;
}

const CollapsibleFieldSection = ({
  title,
  count,
  defaultOpen = true,
  children
}: CollapsibleFieldSectionProps) => {
  const [isOpen, setIsOpen] = useState(defaultOpen);

  return (
    <SidebarMenuItem>
      <SidebarMenuButton
        onClick={() => setIsOpen(!isOpen)}
        className='text-sidebar-foreground hover:bg-sidebar-accent'
      >
        {isOpen ? <ChevronDown /> : <ChevronRight />}
        <span>
          {title} ({count})
        </span>
      </SidebarMenuButton>
      {isOpen && <SidebarMenuSub className='ml-[15px]'>{children}</SidebarMenuSub>}
    </SidebarMenuItem>
  );
};

export default CollapsibleFieldSection;
