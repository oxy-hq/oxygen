import { ArrowUpRight, ChevronDown, House } from "lucide-react";
import { Link } from "react-router-dom";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator
} from "@/components/ui/shadcn/breadcrumb";
import { Button } from "@/components/ui/shadcn/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import { Separator } from "@/components/ui/shadcn/separator";
import { SidebarTrigger } from "@/components/ui/shadcn/sidebar";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import ROUTES from "@/libs/utils/routes";
import { RoleBadge } from "@/pages/admin/components/RoleBadge";
import { usePartnerConsole } from "../../context";

/**
 * The console header: thin (h-9) and dense, matching the admin topbar exactly —
 * this is the same kind of operations surface, so it should not invent its own
 * chrome.
 *
 * The **"Go to <partner>"** button on the right is the load-bearing affordance
 * here. A partner is a real organization with its own Oxy — its people build their
 * own dashboards, ask their own questions — and the console is only the part of
 * their day spent on *clients*. Without a way back to their own product, the
 * console reads as the whole product, which is wrong. It is the one bridge between
 * "administering others" and "using Oxy yourself", so it lives in the chrome and
 * follows you across every page.
 *
 * There is deliberately NO equivalent button for a *client* org: a partner is not
 * a member of their clients' orgs, so opening one would resolve to a 403. A door
 * that doesn't open is worse than no door.
 */
export function PartnerTopbar({ title }: { title: string }) {
  const { partners, active, select } = usePartnerConsole();

  return (
    <header className='flex h-9 shrink-0 items-center gap-1 border-b bg-background px-2'>
      <SidebarTrigger className='-ml-0.5 size-7' />
      <Tooltip>
        <TooltipTrigger asChild>
          <Button asChild variant='ghost' size='icon' className='size-7 text-muted-foreground'>
            <Link to={ROUTES.ROOT} aria-label='Back to home' data-testid='partner-topbar-home'>
              <House className='size-4' />
            </Link>
          </Button>
        </TooltipTrigger>
        <TooltipContent className={DESIGN_REFERENCE} side='bottom'>
          Back to home
        </TooltipContent>
      </Tooltip>
      <Separator orientation='vertical' className='mx-1 h-4' />

      <Breadcrumb>
        <BreadcrumbList className='gap-1 sm:gap-1'>
          <BreadcrumbItem className='text-muted-foreground text-xs'>
            {partners.length > 1 ? (
              <DropdownMenu>
                <DropdownMenuTrigger
                  className='flex items-center gap-0.5 hover:text-foreground'
                  data-testid='partner-topbar-switch'
                >
                  {active.name}
                  <ChevronDown className='size-3' />
                </DropdownMenuTrigger>
                <DropdownMenuContent className={DESIGN_REFERENCE} align='start'>
                  {partners.map((p) => (
                    <DropdownMenuItem key={p.partner_id} onSelect={() => select(p.partner_id)}>
                      {p.name}
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>
            ) : (
              active.name
            )}
          </BreadcrumbItem>
          <BreadcrumbSeparator />
          <BreadcrumbItem>
            <BreadcrumbPage className='font-medium text-xs'>{title}</BreadcrumbPage>
          </BreadcrumbItem>
        </BreadcrumbList>
      </Breadcrumb>

      <div className='ml-auto flex items-center gap-2'>
        {/* What authority am I wielding here? Never leave that implicit. */}
        <RoleBadge kind='partner_operator' />
        <Separator orientation='vertical' className='h-4' />
        <Button asChild variant='outline' size='sm' className='h-7 gap-1 text-xs'>
          <Link to={`/${active.slug}`} data-testid='partner-topbar-go-to-org'>
            Go to {active.name}
            <ArrowUpRight className='size-3.5' />
          </Link>
        </Button>
      </div>
    </header>
  );
}
