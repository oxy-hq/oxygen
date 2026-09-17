import { Table, TableBody } from "@/components/ui/shadcn/table";
import { groupCheckState, type SortDir, type SortKey } from "../useAppsTable";
import { AppGroupHeader } from "./AppGroupHeader";
import type { AppsViewProps } from "./AppsGallery";
import { AppsTableHeader } from "./AppsTableHeader";
import { AppTableRow } from "./AppTableRow";

interface AppsListViewProps extends AppsViewProps {
  sortKey: SortKey;
  sortDir: SortDir;
  onSort: (key: SortKey) => void;
  allSelected: boolean;
  someSelected: boolean;
  onToggleAll: () => void;
}

/**
 * Dense list view: a scrolling table with a sticky sortable header and one
 * `<tbody>` per group. Same selection + actions as the gallery, just packed
 * tighter for scanning many apps at once.
 */
export const AppsListView = ({
  groups,
  statusOf,
  health,
  selecting,
  showOrg,
  showGroupHeaders,
  collapsed,
  onToggleCollapse,
  isSelected,
  onToggleRow,
  onToggleGroup,
  onOpen,
  onPublish,
  onUnpublish,
  sortKey,
  sortDir,
  onSort,
  allSelected,
  someSelected,
  onToggleAll
}: AppsListViewProps) => {
  // checkbox · app · [org] · status · requests · last active · actions
  const colCount = showOrg ? 7 : 6;
  return (
    <Table
      containerClassName='min-h-0 flex-1 overflow-auto'
      className='text-xs'
      data-testid='admin-apps-list'
    >
      <AppsTableHeader
        showOrg={showOrg}
        sortKey={sortKey}
        sortDir={sortDir}
        onSort={onSort}
        allSelected={allSelected}
        someSelected={someSelected}
        onToggleAll={onToggleAll}
      />
      {groups.map((group) => {
        const groupIds = group.items.map((a) => a.id);
        const checked = groupCheckState(groupIds, isSelected);
        const isCollapsed = collapsed.has(group.key);
        return (
          <TableBody key={group.key}>
            {showGroupHeaders && (
              <AppGroupHeader
                label={group.label}
                count={group.items.length}
                collapsed={isCollapsed}
                onToggleCollapse={() => onToggleCollapse(group.key)}
                checked={checked}
                onToggleGroup={() => onToggleGroup(groupIds, checked !== true)}
                labelColSpan={colCount - 1}
              />
            )}
            {!isCollapsed &&
              group.items.map((app) => (
                <AppTableRow
                  key={app.id}
                  app={app}
                  status={statusOf(app)}
                  health={health?.get(app.id)}
                  showOrg={showOrg}
                  isSelected={isSelected(app.id)}
                  selecting={selecting}
                  onToggle={(shiftKey) => onToggleRow(app.id, shiftKey)}
                  onOpen={onOpen}
                  onPublish={onPublish}
                  onUnpublish={onUnpublish}
                />
              ))}
          </TableBody>
        );
      })}
    </Table>
  );
};
