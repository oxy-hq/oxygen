import { House } from "lucide-react";
import { Link } from "react-router-dom";
import { VersionBadge } from "@/components/settings/SettingsDialog/components/VersionBadge";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator
} from "@/components/ui/shadcn/breadcrumb";
import { Button } from "@/components/ui/shadcn/button";
import { Separator } from "@/components/ui/shadcn/separator";
import { SidebarTrigger } from "@/components/ui/shadcn/sidebar";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import ROUTES from "@/libs/utils/routes";
import { AdminEntitySearch } from "../../components/AdminEntitySearch";
import { ADMIN_NAV_GROUPS, type AdminNavGroup } from "../adminNav";
import { AdminIdentity } from "./AdminIdentity";
import { SystemStatus } from "./SystemStatus";

interface AdminTopbarProps {
  title: string;
  /** The rail group this page sits under, or null for a page outside the rail. */
  group: AdminNavGroup | null;
}

/**
 * Global admin header. Kept deliberately thin (h-9) and dense — the admin is a
 * VSCode-like operations surface, so chrome earns its pixels. Left: sidebar
 * toggle, a back-to-home button (the only "exit admin" affordance), and the
 * breadcrumb. Right: live status, entity search, and the build badge (the
 * one-glance "which oxy build am I on?" answer when triaging staging vs prod), and
 * the operator's own identity + platform role — see AdminIdentity.
 */
export function AdminTopbar({ title, group }: AdminTopbarProps) {
  return (
    <header className='flex h-9 shrink-0 items-center gap-1 border-b bg-background px-2'>
      <SidebarTrigger className='-ml-0.5 size-7' />
      <Tooltip>
        <TooltipTrigger asChild>
          <Button asChild variant='ghost' size='icon' className='size-7 text-muted-foreground'>
            <Link to={ROUTES.ROOT} aria-label='Back to home'>
              <House className='size-4' />
            </Link>
          </Button>
        </TooltipTrigger>
        <TooltipContent side='bottom'>Back to home</TooltipContent>
      </Tooltip>
      <Separator orientation='vertical' className='mx-1 h-4' />
      <Breadcrumb>
        <BreadcrumbList className='gap-1 sm:gap-1'>
          <BreadcrumbItem className='text-muted-foreground text-xs'>Admin</BreadcrumbItem>
          <BreadcrumbSeparator />
          {/* The group, so the breadcrumb states the whole position. The pages used to
              print it themselves as an eyebrow — a third copy of the title line, in a
              third wording. */}
          {group ? (
            <>
              <BreadcrumbItem className='hidden text-muted-foreground text-xs sm:block'>
                {ADMIN_NAV_GROUPS[group]}
              </BreadcrumbItem>
              <BreadcrumbSeparator className='hidden sm:block' />
            </>
          ) : null}
          <BreadcrumbItem>
            <BreadcrumbPage className='font-medium text-xs'>{title}</BreadcrumbPage>
          </BreadcrumbItem>
        </BreadcrumbList>
      </Breadcrumb>
      <div className='ml-auto flex items-center gap-2'>
        <SystemStatus />
        <Separator orientation='vertical' className='hidden h-4 md:block' />
        <AdminEntitySearch />
        <Separator orientation='vertical' className='h-4' />
        {/* Who is wielding this surface, and with what authority. Cross-tenant
            power should never be anonymous. */}
        <AdminIdentity />
        <Separator orientation='vertical' className='h-4' />
        <VersionBadge />
      </div>
    </header>
  );
}
