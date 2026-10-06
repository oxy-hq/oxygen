import { formatDistanceToNowStrict } from "date-fns";
import type React from "react";

/** "3 hours ago", with the exact local timestamp on hover. */
export const RelativeTime: React.FC<{ iso: string; className?: string }> = ({ iso, className }) => {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return <span className={className}>Unknown time</span>;
  return (
    <time dateTime={iso} title={date.toLocaleString()} className={className}>
      {formatDistanceToNowStrict(date, { addSuffix: true })}
    </time>
  );
};
