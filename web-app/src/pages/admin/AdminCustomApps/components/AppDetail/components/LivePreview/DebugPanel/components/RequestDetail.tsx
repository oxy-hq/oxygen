import { X } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/shadcn/tabs";
import { cn } from "@/libs/shadcn/utils";
import { isFunctionCallUrl, type OxyRequestEntry } from "../../useOxyRequestLog";
import { getHeader, prettyBody, statusLabel, statusTone } from "../format";
import { ServerSide } from "./ServerSide";

/**
 * DevTools-style detail pane for a single captured request. Three tabs:
 * Headers (general + request/response headers), Payload (query params +
 * request body), and Response (response body). Bodies are pretty-printed as
 * JSON when they parse. A call to one of the app's functions also gets Server
 * side: the `ctx.*` calls of the invocation it ran.
 */
export const RequestDetail = ({
  entry,
  appId,
  onClose,
  className
}: {
  entry: OxyRequestEntry;
  /** The previewed app, for its invocations' call traces. */
  appId?: string;
  onClose: () => void;
  className?: string;
}) => {
  // Set only for a function call: the app whose invocation it ran.
  const serverApp = appId != null && isFunctionCallUrl(entry.url) ? appId : null;
  const params = parseQuery(entry.url);
  const reqType = getHeader(entry.reqHeaders, "content-type");
  const resType = getHeader(entry.resHeaders, "content-type");

  return (
    <div
      className={cn("flex min-w-0 flex-col bg-background", className)}
      data-testid='admin-app-debug-detail'
    >
      <div className='flex items-center gap-2 border-border/60 border-b px-3 py-1.5'>
        <span className='font-medium font-mono text-muted-foreground text-xs'>{entry.method}</span>
        <span className={cn("font-mono text-xs tabular-nums", statusTone(entry))}>
          {statusLabel(entry)}
        </span>
        <span className='truncate font-mono text-foreground text-xs' title={entry.url}>
          {entry.path}
        </span>
        <Button
          variant='ghost'
          size='icon'
          className='ml-auto h-6 w-6 shrink-0 text-muted-foreground'
          onClick={onClose}
          aria-label='Close request detail'
          data-testid='admin-app-debug-detail-close'
        >
          <X className='h-3.5 w-3.5' />
        </Button>
      </div>

      <Tabs defaultValue='headers' className='flex min-h-0 flex-1 flex-col gap-0'>
        <TabsList className='h-8 w-full justify-start rounded-none border-border/60 border-b bg-transparent px-2'>
          <TabsTrigger value='headers' className='text-xs'>
            Headers
          </TabsTrigger>
          <TabsTrigger value='payload' className='text-xs'>
            Payload
          </TabsTrigger>
          <TabsTrigger value='response' className='text-xs'>
            Response
          </TabsTrigger>
          {serverApp != null && (
            <TabsTrigger
              value='server'
              className='text-xs'
              data-testid='admin-app-debug-server-side-tab'
            >
              Server side
            </TabsTrigger>
          )}
        </TabsList>

        <TabsContent value='headers' className='min-h-0 flex-1 overflow-auto p-3'>
          <Section title='General'>
            <KeyValueList
              data={[
                ["Request URL", entry.url],
                ["Method", entry.method],
                ["Status", statusLabel(entry)]
              ]}
            />
          </Section>
          <Section title='Response Headers'>
            <KeyValueList data={Object.entries(entry.resHeaders ?? {})} />
          </Section>
          <Section title='Request Headers'>
            <KeyValueList data={Object.entries(entry.reqHeaders ?? {})} />
          </Section>
        </TabsContent>

        <TabsContent value='payload' className='min-h-0 flex-1 overflow-auto p-3'>
          <Section title='Query String Parameters'>
            <KeyValueList data={params} />
          </Section>
          <Section title='Request Body'>
            <BodyBlock
              text={prettyBody(entry.reqBody, reqType)}
              truncated={entry.reqBodyTruncated}
            />
          </Section>
        </TabsContent>

        <TabsContent value='response' className='min-h-0 flex-1 overflow-auto p-3'>
          <Section title='Response Body'>
            <BodyBlock
              text={prettyBody(entry.resBody, resType)}
              truncated={entry.resBodyTruncated}
            />
          </Section>
        </TabsContent>

        {serverApp != null && (
          <TabsContent
            value='server'
            className='min-h-0 flex-1 overflow-auto p-3'
            data-testid='admin-app-debug-server-side'
          >
            <ServerSide appId={serverApp} entry={entry} />
          </TabsContent>
        )}
      </Tabs>
    </div>
  );
};

function parseQuery(url: string): [string, string][] {
  try {
    return [...new URL(url, window.location.href).searchParams.entries()];
  } catch {
    return [];
  }
}

const Section = ({ title, children }: { title: string; children: React.ReactNode }) => (
  <div className='mb-4 last:mb-0'>
    <h4 className='mb-1.5 font-medium font-mono text-[11px] text-muted-foreground uppercase tracking-wider'>
      {title}
    </h4>
    {children}
  </div>
);

const KeyValueList = ({ data }: { data: [string, string][] }) => {
  if (data.length === 0) return <p className='font-mono text-muted-foreground/60 text-xs'>None</p>;
  return (
    <dl className='grid grid-cols-[minmax(6rem,auto)_1fr] gap-x-3 gap-y-1'>
      {data.map(([k, v]) => (
        <div key={k} className='contents'>
          <dt className='break-words font-medium font-mono text-[11px] text-muted-foreground'>
            {k}
          </dt>
          <dd className='break-all font-mono text-[11px] text-foreground'>{v}</dd>
        </div>
      ))}
    </dl>
  );
};

const BodyBlock = ({ text, truncated }: { text: string; truncated?: boolean }) => {
  if (!text) return <p className='font-mono text-muted-foreground/60 text-xs'>No body</p>;
  return (
    <div>
      <pre className='overflow-auto whitespace-pre-wrap break-words bg-muted/40 p-2 font-mono text-[11px] text-foreground'>
        {text}
      </pre>
      {truncated && (
        <p className='mt-1 font-mono text-[11px] text-muted-foreground/60'>
          (truncated for display)
        </p>
      )}
    </div>
  );
};
