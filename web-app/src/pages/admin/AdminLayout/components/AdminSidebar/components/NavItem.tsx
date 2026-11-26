import { Link } from "react-router-dom";
import { SidebarMenuButton, SidebarMenuItem } from "@/components/ui/shadcn/sidebar";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { consoleNavRow } from "@/pages/admin/components/ConsoleRail";

/** What the rail needs to know about workspace health, resolved once by the rail. */
export interface NavAttention {
  /** Workspaces not reporting healthy; `0` means the item carries no count. */
  count: number;
  /** At least one is `unhealthy` rather than merely degraded — danger, not warning. */
  severe: boolean;
}

/**
 * One entry in the admin rail: a mark, a name, and — on Workspace health — how many
 * workspaces want a person.
 *
 * Its own file because it has to say that count three ways, one per state the rail can
 * be in, and inlining all three put the enclosing `map` callback at cognitive 22 against
 * a limit of 15 (`just metrics`, caught in CI). The three:
 *
 * - **Open:** a pill after the name, toned by whether anything is actually unhealthy.
 * - **Collapsed:** a dot on the mark. The button is a 32px square at that width, so the
 *   number would be clipped to a sliver — which reads as a rendering fault rather than
 *   as a count. The dot sits on the *item*, not inside the button, because the button
 *   clips its own overflow.
 * - **Either, on hover:** the tooltip, which is the only one of the three that can say
 *   the number while collapsed. shadcn renders it only when the rail is collapsed.
 */
export const NavItem = ({
  to,
  label,
  icon: Icon,
  isActive,
  attention
}: {
  to: string;
  label: string;
  icon: React.ComponentType;
  isActive: boolean;
  /** Absent on every item but Workspace health. */
  attention?: NavAttention;
}) => {
  const badge = attention && attention.count > 0 ? attention : null;
  const tone = badge?.severe ? ADMIN_TONE.danger : ADMIN_TONE.warn;

  return (
    <SidebarMenuItem>
      <SidebarMenuButton
        asChild
        isActive={isActive}
        tooltip={badge ? `${label} — ${badge.count} need attention` : label}
        className={consoleNavRow}
      >
        <Link to={to}>
          <Icon />
          <span className='tracking-tight'>{label}</span>
          {badge && (
            <span
              data-testid='workspace-health-nav-badge'
              title={`${badge.count} workspace${badge.count === 1 ? "" : "s"} need attention`}
              className={cn(
                "ml-auto inline-flex h-4 min-w-4 items-center justify-center rounded-full px-1 font-medium text-[10px] tabular-nums ring-1 ring-inset",
                "group-data-[collapsible=icon]:hidden",
                tone.bg,
                tone.text,
                tone.ring
              )}
            >
              {badge.count}
            </span>
          )}
        </Link>
      </SidebarMenuButton>
      {badge && (
        <span
          aria-hidden
          data-testid='workspace-health-nav-dot'
          className={cn(
            "pointer-events-none absolute top-1 right-1 hidden size-1.5 rounded-full group-data-[collapsible=icon]:block",
            tone.dot
          )}
        />
      )}
    </SidebarMenuItem>
  );
};
