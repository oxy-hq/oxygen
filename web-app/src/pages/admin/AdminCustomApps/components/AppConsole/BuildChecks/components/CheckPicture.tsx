import { ImageOff } from "lucide-react";
import { useRef } from "react";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useBuildCheckFile } from "@/hooks/api/customApps/useBuildCheckFile";
import { useSeenOnce } from "../../AppMap/useSeenOnce";
import type { ChecksScope } from "../checksScope";

/**
 * One picture a runner took: a caption, then the picture at its own proportions, as
 * wide as its frame. It is read only once its frame is near the viewport, and it
 * opens full size in a new tab. A picture that was never uploaded (or went with its
 * build) says "Missing" in the frame; a read that failed says so; neither shows a
 * broken image.
 */
export const CheckPicture = ({
  scope,
  attempt,
  name,
  label,
  alt
}: {
  scope: ChecksScope;
  attempt: number;
  /** The file's name, or `null` when the check named none. */
  name: string | null;
  label: string;
  alt: string;
}) => {
  const frame = useRef<HTMLElement>(null);
  const near = useSeenOnce(frame);
  const file = useBuildCheckFile(
    scope.appId,
    scope.build,
    name,
    { attempt, environment: scope.environment },
    near
  );
  return (
    <figure
      ref={frame}
      className='flex min-w-0 flex-col gap-1'
      data-testid='build-check-picture'
      data-state={file.state}
      data-file={name ?? ""}
    >
      <figcaption className='truncate text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
        {label}
      </figcaption>
      {file.state === "ready" && file.url ? (
        <a href={file.url} target='_blank' rel='noreferrer' className='block'>
          <img
            src={file.url}
            alt={alt}
            loading='lazy'
            decoding='async'
            className='block h-auto w-full rounded-sm border border-border/60'
          />
        </a>
      ) : file.state === "missing" || file.state === "error" ? (
        <div className='flex aspect-video items-center justify-center gap-1.5 rounded-sm border border-border/60 border-dashed text-muted-foreground text-xs'>
          <ImageOff className='size-3.5 shrink-0' aria-hidden />
          {file.state === "missing" ? "Missing" : "Couldn't load"}
        </div>
      ) : (
        <Skeleton className='aspect-video w-full rounded-sm' />
      )}
    </figure>
  );
};
