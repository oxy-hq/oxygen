import type React from "react";
import { useId } from "react";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { standingAdds, standingWords } from "@/libs/agentToken";
import type { AgentReview } from "../agentReview";
import type { AgentAsk } from "../cliAuthRequest";
import { LifetimeValue, NameValue, Row } from "./MintRows";

interface StandingProps {
  review: AgentReview;
  included: boolean;
  onIncludedChange: (included: boolean) => void;
  /** The approval is in flight: what was ticked has already been sent. */
  disabled: boolean;
}

/**
 * The approver's staff or partner standing, which the token carries only when oxyc asked for it,
 * they hold some, and they tick the box themselves. The box starts empty: what an agent asks for
 * is said in a sentence, and never granted by a default.
 *
 * Every other case is a sentence alone. Not asked for, there is no box at all: the page never
 * offers more than was asked.
 */
const Standing: React.FC<StandingProps> = ({ review, included, onIncludedChange, disabled }) => {
  const id = useId();
  const { asked, held } = review.standing;
  const words = standingWords(held);

  if (!asked) {
    // Said to someone who holds some, since they are the one who would wonder.
    return held.length > 0 ? (
      <p className='mt-0.5 text-muted-foreground' data-testid='cli-auth-agent-standing-out'>
        Your {words} access is not included.
      </p>
    ) : null;
  }
  if (held.length === 0) {
    return (
      <p className='mt-0.5 text-muted-foreground' data-testid='cli-auth-agent-standing-none'>
        Staff or partner access was asked for. You hold neither, so the token will carry none.
      </p>
    );
  }
  if (!review.approvable) {
    return (
      <p className='mt-0.5 text-muted-foreground'>
        The agent asked to include your {words} access.
      </p>
    );
  }
  return (
    <>
      {/* Two statements, a line each: that it was asked for, and that asking granted nothing. */}
      <p className='mt-0.5' data-testid='cli-auth-agent-standing-asked'>
        The agent asked to include your {words} access.{" "}
        <span className='block'>It is off unless you tick it.</span>
      </p>
      <div className='mt-2 flex items-start gap-2.5'>
        <Checkbox
          id={id}
          className='mt-0.5'
          checked={included}
          onCheckedChange={(on) => onIncludedChange(on === true)}
          disabled={disabled}
          data-testid='cli-auth-agent-standing'
        />
        <label htmlFor={id} className='flex cursor-pointer flex-col'>
          <span className='font-medium'>Include my {words} access</span>
          <span className='text-muted-foreground' data-testid='cli-auth-agent-standing-adds'>
            {standingAdds(held)}
          </span>
        </label>
      </div>
    </>
  );
};

interface Props extends StandingProps {
  /** The request as oxyc sent it, for the one value a review can't read back. */
  ask: AgentAsk;
}

/**
 * What an agent token request asks for, laid out to be read before it is approved: the name the
 * token will carry, what it reaches and the lifetime. Where a sandbox agent token lists apps,
 * this says the whole of the approver's reach, since that is what the token gets.
 */
const AgentRequestSummary: React.FC<Props> = ({ ask, review, ...standing }) => (
  <dl className='mt-6 text-sm leading-5.5' data-testid='cli-auth-agent'>
    <Row label='Name'>
      <NameValue name={review.name} problem={review.problems.name} />
    </Row>
    <Row label='Reaches'>
      <div data-testid='cli-auth-agent-reach'>
        <p className='font-medium'>
          Everything you can reach through your organization memberships
        </p>
        <Standing review={review} {...standing} />
      </div>
    </Row>
    <Row label='Lifetime'>
      <LifetimeValue asked={ask.hours} hours={review.hours} problem={review.problems.hours} />
    </Row>
  </dl>
);

export default AgentRequestSummary;
