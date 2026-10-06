import { CircleAlert } from "lucide-react";
import type React from "react";
import AppSlug from "@/components/ui/AppSlug";
import { lifetimeLabel, sandboxAppRef, sandboxExpiry } from "@/libs/sandboxAgentToken";
import { ApiKeyService } from "@/services/api/apiKey";
import type { MintAsk } from "../cliAuthRequest";
import type { MintAppLine, MintReview } from "../mintReview";

interface Props {
  /** The request as oxyc sent it, for the one value a review can't read back. */
  ask: MintAsk;
  review: MintReview;
}

const Row: React.FC<React.PropsWithChildren<{ label: string }>> = ({ label, children }) => (
  <div className='flex py-2'>
    <dt className='w-24 shrink-0 text-muted-foreground'>{label}</dt>
    <dd className='min-w-0 flex-1'>{children}</dd>
  </div>
);

/**
 * A value that can't be granted, shown as asked and marked where it stands: the mark hangs in
 * the gutter, so the value keeps the edge every other value sits on.
 */
const Refused: React.FC<React.PropsWithChildren<{ why: string }>> = ({ why, children }) => (
  <>
    <div className='relative font-medium text-destructive'>
      <CircleAlert aria-hidden='true' className='absolute top-1 -left-5.5 size-3.5' />
      <span className='sr-only'>Can't be granted: </span>
      {children}
    </div>
    <p className='mt-0.5'>{why}</p>
  </>
);

/** One app oxyc named: its reference beside its name and org, or the reference and why not. */
const AppLine: React.FC<{ line: MintAppLine }> = ({ line }) => (
  <li
    className={line.app ? "flex flex-wrap gap-x-4" : undefined}
    data-testid='cli-auth-mint-app'
    data-app-ref={line.ref}
    data-resolved={line.app ? "true" : "false"}
  >
    {line.app ? (
      <>
        <AppSlug slug={sandboxAppRef(line.app)} className='min-w-40 break-all' />
        <span className='text-muted-foreground'>
          {line.app.name} in {line.app.org_name}
        </span>
      </>
    ) : (
      <Refused why='Not found, or not one you can create a token for.'>
        <AppSlug slug={line.ref} tone='inherit' className='break-all' />
      </Refused>
    )}
  </li>
);

const Apps: React.FC<{ review: MintReview }> = ({ review }) => {
  const count = review.apps.length;
  return (
    <ul className='flex flex-col gap-1.5'>
      {review.apps.map((line) => (
        <AppLine key={line.ref} line={line} />
      ))}
      {review.problems.apps && (
        <li data-testid='cli-auth-mint-app-count'>
          <Refused why={review.problems.apps}>
            {count === 0 ? "None named" : `${count} apps`}
          </Refused>
        </li>
      )}
    </ul>
  );
};

/** How long the token would live, and until when. One that can't be granted is shown as asked. */
const Lifetime: React.FC<Props> = ({ ask, review }) => {
  const { hours, problems } = review;
  if (hours === null || problems.hours) {
    return (
      <Refused why={problems.hours ?? ""}>
        {hours === null ? `"${ask.hours ?? ""}"` : lifetimeLabel(hours)}
      </Refused>
    );
  }
  return (
    <div className='flex flex-wrap gap-x-4'>
      <span className='min-w-40 font-medium'>{lifetimeLabel(hours)}</span>
      <span className='text-muted-foreground'>
        until {ApiKeyService.formatDate(sandboxExpiry(hours).toISOString())}
      </span>
    </div>
  );
};

/**
 * What a CLI mint asks for, laid out to be read before it is approved: the name the token will
 * carry, each app and the lifetime. A value that can't be granted is marked in place.
 */
const MintRequestSummary: React.FC<Props> = ({ ask, review }) => (
  <dl className='mt-6 text-sm leading-5.5' data-testid='cli-auth-mint'>
    <Row label='Name'>
      <div className='break-words' data-testid='cli-auth-mint-name'>
        {review.problems.name ? (
          <Refused why={review.problems.name}>{review.name}</Refused>
        ) : (
          <span className='font-medium'>{review.name}</span>
        )}
      </div>
    </Row>
    <Row label='Apps'>
      <Apps review={review} />
    </Row>
    <Row label='Lifetime'>
      <div data-testid='cli-auth-mint-lifetime'>
        <Lifetime ask={ask} review={review} />
      </div>
    </Row>
  </dl>
);

export default MintRequestSummary;
