import { Cctv, CloudSun, type LucideIcon, Pizza, Users } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { cn } from "@/libs/shadcn/utils";
import type { AppIntegration, AppIntegrationKind } from "@/services/api/appIntegrations";

interface AppCardProps {
  kind: AppIntegrationKind;
  integration: AppIntegration | null;
  loading: boolean;
  onConfigure: () => void;
  /** The server refuses the write (the IDE is retired); the section says why. */
  configureDisabled?: boolean;
}

interface KindMeta {
  label: string;
  description: string;
  icon: LucideIcon;
}

const META: Record<AppIntegrationKind, KindMeta> = {
  toast: {
    label: "Toast POS",
    description:
      "Receives Toast `order_updated` webhooks and exposes them as a server-sent event stream for downstream consumers.",
    icon: Pizza
  },
  openweathermap: {
    label: "OpenWeatherMap",
    description:
      "Powers the weather layer overlay and per-store current-conditions chips. Free-tier key works.",
    icon: CloudSun
  },
  besttime: {
    label: "BestTime",
    description:
      "Foot-traffic busyness percentages per store, plus the radar heatmap. Free-tier key works.",
    icon: Users
  },
  unifi: {
    label: "UniFi Cameras",
    description:
      "Pulls the UniFi Site Manager device roster (name, model, status) for the camera layer. Uses a Site Manager API key.",
    icon: Cctv
  }
};

export function AppCard({
  kind,
  integration,
  loading,
  onConfigure,
  configureDisabled = false
}: AppCardProps) {
  const meta = META[kind];
  const Icon = meta.icon;
  const connected = integration !== null;

  return (
    <div className='flex flex-col gap-3 rounded-md border border-border bg-card p-4'>
      <div className='flex items-start gap-3'>
        <div className='flex h-9 w-9 shrink-0 items-center justify-center rounded-md bg-muted'>
          <Icon className='h-4 w-4 text-muted-foreground' />
        </div>
        <div className='flex min-w-0 flex-1 flex-col gap-1'>
          <div className='flex items-center gap-2'>
            <h4 className='font-medium text-sm'>{meta.label}</h4>
            <span
              className={cn(
                "rounded-full px-2 py-0.5 text-xs",
                connected ? "bg-primary/15 text-primary" : "bg-muted text-muted-foreground"
              )}
            >
              {connected ? "Connected" : "Not connected"}
            </span>
          </div>
          <p className='text-muted-foreground text-xs leading-relaxed'>{meta.description}</p>
        </div>
      </div>

      {integration && <AppCardDetails integration={integration} />}

      <div className='flex justify-end'>
        <Button
          size='sm'
          variant='outline'
          onClick={onConfigure}
          disabled={loading || configureDisabled}
        >
          {connected ? "Edit" : "Connect"}
        </Button>
      </div>
    </div>
  );
}

function AppCardDetails({ integration }: { integration: AppIntegration }) {
  switch (integration.kind) {
    case "toast":
      return (
        <dl className='grid grid-cols-[auto,1fr] gap-x-3 gap-y-1 text-xs'>
          <dt className='text-muted-foreground'>Secret var</dt>
          <dd className='truncate font-mono'>{integration.webhook_secret_var}</dd>
          <dt className='text-muted-foreground'>Restaurants</dt>
          <dd className='truncate'>
            {integration.restaurant_guids.length === 0
              ? "all"
              : `${integration.restaurant_guids.length} configured`}
          </dd>
        </dl>
      );
    case "openweathermap":
    case "besttime":
    case "unifi":
      return (
        <dl className='grid grid-cols-[auto,1fr] gap-x-3 gap-y-1 text-xs'>
          <dt className='text-muted-foreground'>API key var</dt>
          <dd className='truncate font-mono'>{integration.api_key_var}</dd>
        </dl>
      );
  }
}
