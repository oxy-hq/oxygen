import type { AppHealthRow, CustomApp } from "@/types/apps";
import { type AppGroup, type AppStatus, groupCheckState } from "../useAppsTable";
import { AppCard } from "./AppCard";
import { GroupSectionHeader } from "./GroupSectionHeader";

export interface AppsViewProps {
  groups: AppGroup[];
  /** Each app's status, already resolved against the health index. */
  statusOf: (app: CustomApp) => AppStatus | null;
  /** Health rows by app id — `undefined` until the fleet query answers. */
  health: ReadonlyMap<string, AppHealthRow> | undefined;
  /** Any row is selected. */
  selecting: boolean;
  showOrg: boolean;
  showGroupHeaders: boolean;
  collapsed: Set<string>;
  onToggleCollapse: (key: string) => void;
  isSelected: (id: string) => boolean;
  onToggleRow: (id: string, shiftKey: boolean) => void;
  onToggleGroup: (ids: string[], select: boolean) => void;
  onOpen: (app: CustomApp) => void;
  onPublish: (app: CustomApp) => void;
  onUnpublish: (app: CustomApp) => void;
}

/**
 * Card-grid view. Responsive `1 → 2 → 3 → 4` columns with 8px-scale gaps, one
 * section per group. The whole scroll region lives here so the toolbar and
 * bulk bar stay pinned.
 */
export const AppsGallery = ({
  groups,
  statusOf,
  showOrg,
  showGroupHeaders,
  collapsed,
  onToggleCollapse,
  isSelected,
  onToggleRow,
  onToggleGroup,
  onOpen,
  onPublish,
  onUnpublish
}: AppsViewProps) => (
  <div className='min-h-0 flex-1 space-y-6 overflow-auto p-4'>
    {groups.map((group) => {
      const groupIds = group.items.map((a) => a.id);
      const checked = groupCheckState(groupIds, isSelected);
      const isCollapsed = collapsed.has(group.key);
      return (
        <section key={group.key} className='space-y-3'>
          {showGroupHeaders && (
            <GroupSectionHeader
              label={group.label}
              count={group.items.length}
              collapsed={isCollapsed}
              onToggleCollapse={() => onToggleCollapse(group.key)}
              checked={checked}
              onToggleGroup={() => onToggleGroup(groupIds, checked !== true)}
            />
          )}
          {!isCollapsed && (
            <div className='grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4'>
              {group.items.map((app) => (
                <AppCard
                  key={app.id}
                  app={app}
                  status={statusOf(app)}
                  showOrg={showOrg}
                  isSelected={isSelected(app.id)}
                  onToggle={(shiftKey) => onToggleRow(app.id, shiftKey)}
                  onOpen={onOpen}
                  onPublish={onPublish}
                  onUnpublish={onUnpublish}
                />
              ))}
            </div>
          )}
        </section>
      );
    })}
  </div>
);
