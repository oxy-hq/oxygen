import { TriangleAlert } from "lucide-react";
import type React from "react";

/** "Acme", "Acme and Globex", "Acme, Globex and 3 more". */
export const listNames = (names: string[]): string =>
  names.length <= 2 ? names.join(" and ") : `${names[0]}, ${names[1]} and ${names.length - 2} more`;

/** A caution that doesn't block saving: the token would be created, and not work somewhere. */
const Caution: React.FC<React.PropsWithChildren<{ testId: string }>> = ({ testId, children }) => (
  <p className='flex gap-1.5 text-muted-foreground text-xs' data-testid={testId}>
    <TriangleAlert className='mt-0.5 size-3.5 shrink-0 text-destructive' aria-hidden />
    <span>{children}</span>
  </p>
);

export default Caution;
