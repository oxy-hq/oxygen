import { Loader2 } from "lucide-react";
import { useMemo, useState } from "react";
import { useForm } from "react-hook-form";
import { toast } from "sonner";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/shadcn/alert";
import { Button } from "@/components/ui/shadcn/button";
import {
  useExperimentProposals,
  usePriceExperiment,
  useUseProposal
} from "@/hooks/api/useExperiments";
import { useRole } from "@/hooks/useRole";
import { cn } from "@/libs/shadcn/utils";
import { StatGrid, StatTile } from "@/pages/ide/Experiments/components/glance/StatTile";
import { ToneBadge } from "@/pages/ide/Experiments/components/glance/tone";
import { earliestTodayOnEarth } from "@/pages/ide/Experiments/dates";
import { COLUMN } from "@/pages/ide/Experiments/ExperimentDetail/column";
import { formatUnitDay } from "@/pages/ide/Experiments/format";
import { grainOf } from "@/pages/ide/Experiments/grain";
import type { ExperimentListItem, Proposal } from "@/types/experiment";
import HowCalculated from "../ResultPane/components/HowCalculated";
import RetrospectiveOffer from "../RetrospectiveOffer";
import type { AdoptControl } from "./components/CandidateDetail";
import CandidateForm from "./components/CandidateForm";
import PriceOutcome from "./components/PriceOutcome";
import { draftShape, newCandidate, type PriceFormValues, validatePriceForm } from "./priceForm";
import { type Awaiting, assignmentsProposed, PRICING_TIMEOUT_MS, pricingView } from "./proposals";
import { makePriceResolver } from "./resolver";
import { useNow } from "./useNow";

/**
 * A draft: price it. The form is the price request's *input* — one row per
 * candidate design, none of whose fields are keys in the design. Submitting
 * enqueues a job per candidate and returns the request's proposal ids; the log is
 * polled until *every* row of that request leaves `pending`, and whatever each
 * settles as — priced, refused, faulted — is page content.
 */
export default function PricingPanel({ summary }: { summary: ExperimentListItem }) {
  const price = usePriceExperiment(summary.name);
  const adoptMutation = useUseProposal(summary.name);
  const { is } = useRole();
  const [adoptError, setAdoptError] = useState<string | null>(null);
  const [adopted, setAdopted] = useState(false);
  const [awaiting, setAwaiting] = useState<Awaiting | null>(null);
  const [submitError, setSubmitError] = useState<string | null>(null);
  // A request found pending on load is waited on for the same bounded time as a fresh one.
  const [resumedAt] = useState(() => Date.now());
  const viewAt = (log: Proposal[], at: number) =>
    pricingView({ awaiting, proposals: log, now: at, timeoutMs: PRICING_TIMEOUT_MS, resumedAt });

  const proposalsQuery = useExperimentProposals(summary.name, {
    keepPolling: (log) => viewAt(log ?? [], Date.now()).status === "waiting"
  });
  const proposals = proposalsQuery.data ?? [];
  // The clock ticks only while waiting: it re-renders so the deadline can fire, then stops.
  // The view reads the live time, not the last tick, so the render that ends the wait is current.
  useNow(viewAt(proposals, Date.now()).status === "waiting");
  const view = viewAt(proposals, Date.now());

  const grain = grainOf(summary.definition.design.grain);
  const context = useMemo(
    () => ({ earliestToday: earliestTodayOnEarth(new Date()), grain }),
    [grain]
  );
  const resolver = useMemo(() => makePriceResolver(context), [context]);
  const shape = draftShape(summary.definition.design);
  const form = useForm<PriceFormValues>({
    defaultValues: { candidates: [newCandidate(shape)] },
    resolver
  });

  const onSubmit = form.handleSubmit(async (values) => {
    const { request } = validatePriceForm(values, context);
    if (!request) return;
    setSubmitError(null);
    try {
      const queued = await price.mutateAsync(request);
      setAwaiting({
        requestId: queued.request_id,
        ids: queued.proposal_ids,
        startedAt: Date.now()
      });
    } catch (error) {
      setSubmitError(error instanceof Error ? error.message : "Pricing could not be queued.");
    }
  });

  const adopt: AdoptControl = {
    canWrite: is.workspaceEditor,
    pending: adoptMutation.isPending,
    onUse: async (proposalId) => {
      setAdoptError(null);
      try {
        await adoptMutation.mutateAsync(proposalId);
        setAdopted(true);
        toast.success("Updated the draft design");
      } catch (error) {
        setAdoptError(error instanceof Error ? error.message : "The design could not be used.");
      }
    }
  };

  const assignments = Math.max(assignmentsProposed(proposals), summary.proposals_count);
  const latest = summary.latest_proposal;
  const latestValue =
    latest === null
      ? "Not priced yet"
      : latest.status === "done"
        ? formatUnitDay(latest.mde_unit_day, grain)
        : latest.status === "pending"
          ? "Pricing…"
          : latest.status === "refused"
            ? "Refused"
            : "Failed";
  const settledRows = view.proposals.filter((p) => p.status !== "pending");
  return (
    <div className={cn(COLUMN, "flex flex-col gap-6")} data-testid='experiment-pricing'>
      {/* A design whose dates are past (an observed rollout above all) is read, not priced. */}
      {summary.retrospective_ready && <RetrospectiveOffer summary={summary} />}
      <section className='flex flex-col gap-3'>
        <div className='flex flex-col gap-3'>
          <div className='flex flex-wrap items-center justify-between gap-2'>
            <h2 className='font-semibold text-base'>Price this design</h2>
            <ToneBadge tone='neutral' testId='experiment-pricing-status'>
              Draft
            </ToneBadge>
          </div>
          <p className='text-muted-foreground text-sm'>
            Price several candidates side by side; each reports the smallest effect it could read,
            or refuses.
          </p>
          <StatGrid>
            <StatTile
              id='pricing-design'
              testId='experiment-pricing-design'
              label='Design'
              value='Draft'
            />
            <StatTile
              id='pricing-latest'
              testId='experiment-pricing-latest'
              label='Latest MDE'
              value={latestValue}
              sub={latest?.status === "done" ? `per unit-${grain}` : undefined}
              tone={latest?.status === "done" ? "success" : latest ? "danger" : undefined}
              hint={`The smallest effect the most recent pricing could read, per unit-${grain}.`}
            />
            <StatTile
              id='pricing-assignments'
              label='Assignments'
              value={assignments}
              sub='proposed so far'
            />
          </StatGrid>
          <HowCalculated title='How pricing works'>
            <div data-testid='experiment-pricing-how' className='space-y-1.5'>
              <p>
                Which design can read an effect at all depends on this workspace's own history, so
                price several side by side. Each candidate draws its own randomised assignment and
                reports the smallest effect it could read, or refuses and says why.
              </p>
              <p>
                The report will state how many assignments were drawn before the one you register,
                candidates compared side by side included.
              </p>
            </div>
          </HowCalculated>
        </div>
        {proposalsQuery.isError && (
          <div data-testid='experiment-proposals-error'>
            <ErrorAlert
              title='The proposal log could not be loaded'
              message={proposalsQuery.error.message}
              actions={
                <Button variant='outline' size='sm' onClick={() => proposalsQuery.refetch()}>
                  Retry
                </Button>
              }
            />
          </div>
        )}
        <CandidateForm
          form={form}
          shape={shape}
          onSubmit={onSubmit}
          disabled={!proposalsQuery.isSuccess || !is.workspaceEditor}
          denied={!is.workspaceEditor}
          busy={price.isPending || view.status === "waiting"}
          // The log is one capped page; the list's own count never undercounts.
          assignments={assignments}
          grain={grain}
        />
      </section>

      {submitError && <ErrorAlert title='Pricing was not queued' message={submitError} />}
      {view.status === "waiting" && (
        <div
          role='status'
          className='flex items-center gap-2 text-muted-foreground text-sm'
          data-testid='experiment-price-waiting'
        >
          <Loader2 className='h-4 w-4 animate-spin' />
          <span data-testid='experiment-price-progress'>
            Pricing… {view.settled} of {view.total} candidates done.
          </span>
          Placebo power runs a permutation test per draw and can take a minute or more.
        </div>
      )}
      {view.status === "timed_out" && (
        <Alert data-testid='experiment-price-timeout'>
          <AlertTitle>Pricing is taking longer than expected</AlertTitle>
          <AlertDescription>
            {view.settled} of {view.total} candidates have finished after 3 minutes; the rest may be
            queued behind other jobs. Reload later and their outcomes will be shown here.
          </AlertDescription>
        </Alert>
      )}
      {adopted && (
        <Alert data-testid='experiment-use-design-done'>
          <AlertTitle>Draft design updated</AlertTitle>
          <AlertDescription>
            The priced assignment is now in the draft design. Price it again to compare further
            candidates against it.
          </AlertDescription>
        </Alert>
      )}
      {adoptError && (
        <div data-testid='experiment-use-design-error'>
          <ErrorAlert title='The design was not used' message={adoptError} />
        </div>
      )}
      {view.status !== "waiting" && settledRows.length > 0 && (
        <PriceOutcome proposals={settledRows} adopt={adopt} grain={grain} />
      )}
    </div>
  );
}
