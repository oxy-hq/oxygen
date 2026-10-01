import { Badge } from "@/components/ui/shadcn/badge";
import { cn } from "@/libs/shadcn/utils";
import type { PreviewCheckVerdict } from "@/types/workspace";

const LABEL: Record<PreviewCheckVerdict, string> = {
  additive: "Additive",
  warning: "Warning",
  needs_reset: "Needs reset"
};

/** `needs_reset` uses the theme's destructive variant; the other two share this outline treatment. */
const OUTLINE_CLASS: Record<Exclude<PreviewCheckVerdict, "needs_reset">, string> = {
  additive: "border-success/40 bg-success/10 text-success",
  warning: "border-warning/40 bg-warning/10 text-warning"
};

/** Small verdict badge shared by a pipeline's own verdict and each finding's. */
export default function PreviewVerdictBadge({ verdict }: { verdict: PreviewCheckVerdict }) {
  if (verdict === "needs_reset") {
    return (
      <Badge variant='destructive' data-testid='preview-verdict-badge'>
        {LABEL[verdict]}
      </Badge>
    );
  }
  return (
    <Badge
      variant='outline'
      className={cn(OUTLINE_CLASS[verdict])}
      data-testid='preview-verdict-badge'
    >
      {LABEL[verdict]}
    </Badge>
  );
}
