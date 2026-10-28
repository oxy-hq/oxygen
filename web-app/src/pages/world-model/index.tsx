import { Navigate } from "react-router-dom";
import { Card } from "@/components/ui/shadcn/card";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useWorldModel } from "@/hooks/api/useWorldModel";
import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import { worldModelTitle } from "@/pages/launcher/CustomerHome/components/WorldModelCard";
import { WorldModelCounts } from "@/pages/launcher/CustomerHome/components/WorldModelCard/WorldModelCounts";
import { HOME_CONTAINER } from "@/pages/launcher/CustomerHome/layout";
import useCurrentOrg from "@/stores/useCurrentOrg";
import { WorldModelExplorer } from "./components/WorldModelExplorer";

/**
 * The world model a customer can open from the home: the entities their company's
 * data defines, what is measured about each, and how they connect. Read-only. It
 * draws the Factory's own graph component and reads one customer route
 * (`GET /semantic/world-model`); exploring instances, breakdowns and peers, and
 * changing the model, stay in the Factory.
 *
 * Apps-first only. With the flag off there is no card linking here, and a stray
 * link goes to the home.
 */
export default function CustomerWorldModelPage() {
  const appsFirst = useAppsFirstExperience();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  if (!appsFirst) return <Navigate to={ROUTES.ORG(orgSlug).HOME} replace />;
  return <WorldModelPageContent />;
}

function WorldModelPageContent() {
  const org = useCurrentOrg((s) => s.org);
  const { data: model, isLoading, error } = useWorldModel();
  const ready = !!model && model.entities.length > 0;

  // The home's container and type, so the page keeps the home's edges and the
  // card's title becomes this page's heading. The map sits in one bordered card,
  // as it does on the home.
  return (
    <div className='flex h-full min-h-0 flex-col overflow-auto' data-testid='customer-world-model'>
      <header className={cn(HOME_CONTAINER, "flex flex-col gap-4 pt-10 pb-6")}>
        <div className='flex flex-col gap-1'>
          <h1 className='font-medium text-2xl tracking-tight sm:text-3xl'>
            {worldModelTitle(org?.name)}
          </h1>
          <p className='max-w-prose text-muted-foreground text-sm'>
            The things {org?.name ?? "your company"} keeps track of, what is measured about each,
            and how they connect. Select one to see its details.
          </p>
        </div>
        {ready && <WorldModelCounts model={model} />}
      </header>
      <div className={cn(HOME_CONTAINER, "flex min-h-0 flex-1 flex-col pb-10")}>
        <Card className='flex min-h-96 flex-1 flex-col overflow-hidden shadow-none'>
          {isLoading ? (
            <div className='flex flex-1 items-center justify-center'>
              <Spinner className='size-6' />
            </div>
          ) : error || !model ? (
            <PageNote>This map isn't available right now. Try again in a moment.</PageNote>
          ) : !ready ? (
            <PageNote>Your Oxy team hasn't described how your data fits together yet.</PageNote>
          ) : (
            <WorldModelExplorer model={model} />
          )}
        </Card>
      </div>
    </div>
  );
}

function PageNote({ children }: { children: string }) {
  return (
    <div className='flex flex-1 items-center justify-center px-6 text-center text-muted-foreground text-sm'>
      {children}
    </div>
  );
}
