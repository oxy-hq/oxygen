import { Card } from "@/components/ui/shadcn/card";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useWorldModel } from "@/hooks/api/useWorldModel";
import { cn } from "@/libs/shadcn/utils";
import { possessive } from "@/libs/utils/possessive";
import useCurrentOrg from "@/stores/useCurrentOrg";
import { WorldModelCounts } from "./components/WorldModelCounts";
import { WorldModelExplorer } from "./components/WorldModelExplorer";

/** The frame the page sits in, so a banner above it lines up with the content. */
const PAGE_CONTAINER = "mx-auto w-full max-w-7xl px-6";

/** The page's heading. */
function worldModelTitle(orgName?: string) {
  return orgName ? `How ${possessive(orgName)} data fits together` : "How your data fits together";
}

/**
 * The world model a customer can open from the home: the entities their company's
 * data defines, what is measured about each, and how they connect. Read-only. It
 * draws the Factory's own graph component and reads one customer route
 * (`GET /semantic/world-model`); exploring instances, breakdowns and peers, and
 * changing the model, stay in the Factory.
 */
export default function CustomerWorldModelPage() {
  const org = useCurrentOrg((s) => s.org);
  const { data: model, isLoading, error } = useWorldModel();
  const ready = !!model && model.entities.length > 0;

  // The map sits in one bordered card.
  return (
    <div className='flex h-full min-h-0 flex-col overflow-auto' data-testid='customer-world-model'>
      <header className={cn(PAGE_CONTAINER, "flex flex-col gap-4 pt-10 pb-6")}>
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
      <div className={cn(PAGE_CONTAINER, "flex min-h-0 flex-1 flex-col pb-10")}>
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
