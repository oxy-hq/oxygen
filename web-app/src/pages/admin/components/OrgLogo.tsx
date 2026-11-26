import { useAdminOrgLogo } from "@/hooks/api/adminTenants/useAdminOrgs";
import { cn } from "@/libs/shadcn/utils";

const BOX: Record<"sm" | "md" | "lg", string> = {
  sm: "size-8 text-xs",
  md: "size-10 text-xs",
  lg: "size-16 text-2xl"
};

/**
 * An organization's uploaded logo, with a name-initial fallback. Fetches the
 * bytes through the authenticated admin API (as a data URL), so it works for
 * any tenant an Oxy admin can see — no public image endpoint. Falls back to a
 * neutral monogram while loading or when no logo is set.
 */
export const OrgLogo = ({
  orgId,
  name,
  size = "sm",
  className
}: {
  orgId: string;
  name: string;
  size?: "sm" | "md" | "lg";
  className?: string;
}) => {
  const { data: dataUrl } = useAdminOrgLogo(orgId);
  const box = BOX[size];
  if (dataUrl) {
    return (
      <img
        src={dataUrl}
        alt=''
        className={cn(
          "shrink-0 border border-border-subtle bg-background object-contain",
          box,
          className
        )}
      />
    );
  }
  return (
    <span
      className={cn(
        "flex shrink-0 items-center justify-center border border-border-subtle bg-muted/40 font-semibold text-muted-foreground",
        box,
        className
      )}
    >
      {name.slice(0, 1).toUpperCase()}
    </span>
  );
};
