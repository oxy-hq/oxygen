import { cn } from "@/libs/shadcn/utils";
import type { PlatformRoleId } from "@/types/access";
import type { TenantType } from "../useTenantSelection";

/**
 * Two SEPARATE types, deliberately not a `OrgFilter | UserFilter` union.
 *
 * The union version shipped with a reset effect whose dep array was `[]`, so an org
 * filter survived onto the user list — `"empty"` went out as `?role=empty`, the server
 * correctly resolved an unknown role to an empty allow-list, and the rail came back
 * blank with no chip lit. Two casts (`filter as UserFilter`) were the only reason that
 * typechecked; the type system would otherwise have objected that `"empty"` is not a
 * UserFilter.
 *
 * One state per entity makes that unrepresentable and deletes the reset entirely —
 * there is nothing to reset, because each list reads its own.
 */

/** Orgs: the shapes an operator actually goes looking for. */
export type OrgFilter = "all" | "managed" | "unmanaged" | "empty";
/** Users: staff standing, which is otherwise invisible until you open each row. */
export type UserFilter = "all" | "staff" | PlatformRoleId;

const ORG_FILTERS: { id: OrgFilter; label: string; hint: string }[] = [
  { id: "all", label: "All", hint: "Every organization" },
  { id: "managed", label: "Managed", hint: "Operated by a partner" },
  { id: "unmanaged", label: "Direct", hint: "No partner — Oxy holds the relationship" },
  { id: "empty", label: "Empty", hint: "No members — provisioned but never used" }
];

const USER_FILTERS: { id: UserFilter; label: string; hint: string }[] = [
  { id: "all", label: "All", hint: "Every user" },
  { id: "staff", label: "Staff", hint: "Holds any platform grant" },
  { id: "global_admin", label: "Global Admin", hint: "Full console" },
  { id: "app_operator", label: "App Operator", hint: "Custom apps only" }
];

/**
 * Attribute filters for the rail, beside its text search.
 *
 * Search answers "where is Acme"; these answer the questions an operator actually opens
 * this surface with — which tenants has nobody touched, which are partner-operated, who
 * on this deployment can get into the console. None of those are answerable by typing a
 * name, and all of them were previously a manual scan of every row.
 *
 * Only two entity types get filters. Partners have one meaningful axis (they are all
 * partners) so a filter row there would be chrome.
 */
export function RailFilters<T extends string>({
  type,
  value,
  onChange
}: {
  type: TenantType;
  value: T;
  onChange: (next: T) => void;
}) {
  const filters: { id: string; label: string; hint: string }[] =
    type === "orgs" ? ORG_FILTERS : type === "users" ? USER_FILTERS : [];
  if (filters.length === 0) return null;

  return (
    <div
      className='flex flex-wrap items-center gap-1'
      data-testid={`admin-rail-filters-${type}`}
      role='group'
      aria-label='Filter list'
    >
      {filters.map((f) => (
        <button
          key={f.id}
          type='button'
          title={f.hint}
          onClick={() => onChange(f.id as T)}
          aria-pressed={value === (f.id as string)}
          // Type-qualified: both sets emit an "all" chip, so an unqualified id cannot
          // tell a flow which list it is asserting on.
          data-testid={`admin-rail-filter-${type}-${f.id}`}
          className={cn(
            "px-1.5 py-0.5 font-medium text-xs transition-colors",
            value === (f.id as string)
              ? "bg-foreground text-background"
              : "bg-muted/60 text-muted-foreground hover:text-foreground"
          )}
        >
          {f.label}
        </button>
      ))}
    </div>
  );
}
