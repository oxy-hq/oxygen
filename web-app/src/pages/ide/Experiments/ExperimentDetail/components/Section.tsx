import type { ReactNode } from "react";

/** A titled, bordered block — the unit every detail view is built from. */
export default function Section({
  title,
  testId,
  children
}: {
  title: string;
  testId?: string;
  children: ReactNode;
}) {
  return (
    <section className='rounded-lg border p-4' data-testid={testId}>
      <h3 className='mb-2 font-medium text-sm'>{title}</h3>
      {children}
    </section>
  );
}
