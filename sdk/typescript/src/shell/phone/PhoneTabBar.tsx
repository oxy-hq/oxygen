// The bottom tab bar of a phone view: an app's sections, each one thumb-tap
// away, in place of a sideways-scrolling tab strip. Five slots at most, as on
// iOS and Android; past five, the last slot is "More" and opens the rest in a
// sheet, so no section is ever cut mid-word or scrolled off.

import { type ReactNode, useState } from "react";
import { cx } from "../cx";
import { ChevronRightIcon, MoreIcon } from "./icons";
import { PhoneSheet } from "./PhoneSheet";

export interface PhoneTabItem {
  id: string;
  label: string;
  /** A 22px glyph (any inline SVG or icon component). */
  icon?: ReactNode;
  /** A count or short word on the tab ("3", "New"); 0, "0" and "" show nothing. */
  badge?: number | string;
}

export interface PhoneTabBarProps {
  items: PhoneTabItem[];
  activeId: string;
  onSelect: (id: string) => void;
  moreLabel?: string;
  /** The nav landmark's name, read by screen readers. */
  ariaLabel?: string;
}

const MAX_SLOTS = 5;

function hasBadge(badge: PhoneTabItem["badge"] | null): boolean {
  return badge !== undefined && badge !== null && badge !== 0 && badge !== "" && badge !== "0";
}

function badgeText(badge: number | string): string {
  return typeof badge === "number" && badge > 99 ? "99+" : String(badge);
}

function TabSlot({
  label,
  icon,
  badge,
  dot,
  active,
  expanded,
  onClick
}: {
  label: string;
  icon?: ReactNode;
  badge?: number | string;
  dot?: boolean;
  active: boolean;
  expanded?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type='button'
      className='oxy-phone-tabbar__slot'
      aria-current={active ? "page" : undefined}
      aria-expanded={expanded}
      aria-haspopup={expanded === undefined ? undefined : "dialog"}
      onClick={onClick}
    >
      {icon && (
        <span className='oxy-phone-tabbar__icon' aria-hidden='true'>
          {icon}
        </span>
      )}
      <span className='oxy-phone-tabbar__label'>{label}</span>
      {badge !== undefined && hasBadge(badge) && (
        <span className='oxy-phone-tabbar__badge'>{badgeText(badge)}</span>
      )}
      {dot && (
        <>
          <span className='oxy-phone-tabbar__dot' aria-hidden='true' />
          <span className='oxy-phone-sr'>, has updates</span>
        </>
      )}
    </button>
  );
}

export function PhoneTabBar({
  items,
  activeId,
  onSelect,
  moreLabel = "More",
  ariaLabel = "Sections"
}: PhoneTabBarProps) {
  const [moreOpen, setMoreOpen] = useState(false);
  const overflow = items.length > MAX_SLOTS;
  const slots = overflow ? items.slice(0, MAX_SLOTS - 1) : items;
  const rest = overflow ? items.slice(MAX_SLOTS - 1) : [];
  const restActive = rest.some((item) => item.id === activeId);
  // More carries a dot when anything behind it has a badge: a count summed
  // across unlike sections would mean nothing.
  const restDot = rest.some((item) => hasBadge(item.badge));
  // The items can shrink while More is open: its slot and its list are gone, so
  // it closes rather than stay as an empty sheet, or reopen when they grow back.
  if (moreOpen && !overflow) setMoreOpen(false);

  return (
    <nav aria-label={ariaLabel} className='oxy-shell-scope oxy-phone-tabbar'>
      {slots.map((item) => (
        <TabSlot
          key={item.id}
          label={item.label}
          icon={item.icon}
          badge={item.badge}
          active={item.id === activeId}
          onClick={() => onSelect(item.id)}
        />
      ))}
      {overflow && (
        <TabSlot
          label={moreLabel}
          icon={<MoreIcon />}
          dot={restDot}
          active={restActive}
          expanded={moreOpen}
          onClick={() => setMoreOpen(true)}
        />
      )}
      <PhoneSheet open={moreOpen && overflow} onClose={() => setMoreOpen(false)} title={moreLabel}>
        <div className='oxy-phone-more'>
          {rest.map((item) => {
            const active = item.id === activeId;
            return (
              <button
                key={item.id}
                type='button'
                aria-current={active ? "page" : undefined}
                className={cx("oxy-phone-more__row", active && "oxy-phone-more__row--active")}
                onClick={() => {
                  setMoreOpen(false);
                  onSelect(item.id);
                }}
              >
                {item.icon && (
                  <span className='oxy-phone-more__icon' aria-hidden='true'>
                    {item.icon}
                  </span>
                )}
                <span className='oxy-phone-more__label'>{item.label}</span>
                {item.badge !== undefined && hasBadge(item.badge) && (
                  <span className='oxy-phone-tabbar__badge oxy-phone-more__badge'>
                    {badgeText(item.badge)}
                  </span>
                )}
                <span className='oxy-phone-more__chevron'>
                  <ChevronRightIcon />
                </span>
              </button>
            );
          })}
        </div>
      </PhoneSheet>
    </nav>
  );
}
