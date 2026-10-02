import { useEffect, useState } from "react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/shadcn/popover";
import { getVersion, type VersionInfo } from "@/services/api/version";

export const VersionBadge = () => {
  const [versionInfo, setVersionInfo] = useState<VersionInfo | null>(null);

  useEffect(() => {
    const fetchVersion = async () => {
      try {
        const info = await getVersion();
        setVersionInfo(info);
      } catch (error) {
        console.error("Failed to fetch version:", error);
      }
    };

    void fetchVersion();
  }, []);

  if (!versionInfo) {
    return null;
  }

  const formatDate = (timestamp: string) => {
    const date = new Date(timestamp);
    if (Number.isNaN(date.getTime())) {
      return timestamp;
    }
    return date.toLocaleString("en-US", {
      year: "numeric",
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit"
    });
  };

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type='button'
          className='select-none text-muted-foreground text-xs transition-opacity hover:opacity-80'
        >
          v{versionInfo.version}
        </button>
      </PopoverTrigger>
      <PopoverContent className='w-80' align='start'>
        <div className='space-y-2'>
          <div className='font-semibold text-sm'>Build Information</div>
          <div className='space-y-1 text-xs'>
            <div className='flex justify-between'>
              <span className='text-muted-foreground'>Version:</span>
              <span className='font-mono'>{versionInfo.version}</span>
            </div>
            <div className='flex justify-between'>
              <span className='text-muted-foreground'>Commit:</span>
              {versionInfo.build_info.commit_url ? (
                <a
                  href={versionInfo.build_info.commit_url}
                  target='_blank'
                  rel='noopener noreferrer'
                  className='font-mono text-primary hover:underline'
                >
                  {versionInfo.build_info.git_commit_short}
                </a>
              ) : (
                <span className='font-mono'>{versionInfo.build_info.git_commit_short}</span>
              )}
            </div>
            <div className='flex justify-between'>
              <span className='text-muted-foreground'>Built:</span>
              <span>{formatDate(versionInfo.build_info.build_timestamp)}</span>
            </div>
            <div className='flex justify-between'>
              <span className='text-muted-foreground'>Profile:</span>
              <span className='font-mono'>{versionInfo.build_info.build_profile}</span>
            </div>
            {versionInfo.build_info.workflow_url && (
              <div className='border-t pt-2'>
                <a
                  href={versionInfo.build_info.workflow_url}
                  target='_blank'
                  rel='noopener noreferrer'
                  className='text-primary text-xs hover:underline'
                >
                  View build workflow →
                </a>
              </div>
            )}
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
};
