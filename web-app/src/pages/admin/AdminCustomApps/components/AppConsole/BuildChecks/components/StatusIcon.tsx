import {
  CircleCheck,
  CircleDashed,
  CircleMinus,
  CircleX,
  Loader2,
  TriangleAlert
} from "lucide-react";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import { checkStatus } from "../buildChecksModel";

/** A status as a glyph, so a column of rows (or a row of steps) reads at a glance
 *  before any word does. */
export const StatusIcon = ({ status }: { status: string }) => {
  const tone = ADMIN_TONE[checkStatus(status).tone].text;
  const cls = cn("size-4 shrink-0", tone);
  switch (status) {
    case "passed":
      return <CircleCheck className={cls} aria-hidden />;
    case "failed":
      return <CircleX className={cls} aria-hidden />;
    case "error":
      return <TriangleAlert className={cls} aria-hidden />;
    case "running":
      return <Loader2 className={cn(cls, "animate-spin motion-reduce:animate-none")} aria-hidden />;
    case "skipped":
      return <CircleMinus className={cls} aria-hidden />;
    default:
      return <CircleDashed className={cls} aria-hidden />;
  }
};
