import type React from "react";
import AppSlug from "@/components/ui/AppSlug";
import { sandboxAppRef } from "@/libs/sandboxAgentToken";
import type { MintAsk } from "../cliAuthRequest";
import type { MintAppLine, MintReview } from "../mintReview";
import { LifetimeValue, NameValue, Refused, Row } from "./MintRows";

interface Props {
  /** The request as oxyc sent it, for the one value a review can't read back. */
  ask: MintAsk;
  review: MintReview;
}

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

/**
 * What a sandbox agent mint asks for, laid out to be read before it is approved: the name the
 * token will carry, each app and the lifetime. A value that can't be granted is marked in place.
 */
const MintRequestSummary: React.FC<Props> = ({ ask, review }) => (
  <dl className='mt-6 text-sm leading-5.5' data-testid='cli-auth-mint'>
    <Row label='Name'>
      <NameValue name={review.name} problem={review.problems.name} />
    </Row>
    <Row label='Apps'>
      <Apps review={review} />
    </Row>
    <Row label='Lifetime'>
      <LifetimeValue asked={ask.hours} hours={review.hours} problem={review.problems.hours} />
    </Row>
  </dl>
);

export default MintRequestSummary;
