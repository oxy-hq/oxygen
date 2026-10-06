import type React from "react";
import { cn } from "@/libs/shadcn/utils";
import "./TableWrapper.css";

/**
 * Wraps a settings table in a bordered card. On md+ it stays a horizontally
 * scrollable table; below md the table collapses into stacked cards via the
 * sibling stylesheet — each `<td>` becomes a labeled block using its
 * `data-label` attribute as the field name.
 *
 * Action cells (or any cells that should render without a header) just omit
 * `data-label`.
 */
const TableWrapper: React.FC<React.PropsWithChildren<{ plain?: boolean }>> = ({
  plain,
  children
}) => {
  return (
    <div
      className={cn(
        "settings-table-wrapper w-full md:overflow-x-auto",
        // `plain` leaves the card off, for a table ruled by its own hairlines.
        !plain && "rounded-lg border"
      )}
    >
      {children}
    </div>
  );
};

export default TableWrapper;
