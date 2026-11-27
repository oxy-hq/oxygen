import { Button } from "@/components/ui/shadcn/button";
import type { BuildChecksEnvironment } from "@/types/appBuildChecks";

const ENVIRONMENTS: { value: BuildChecksEnvironment; label: string }[] = [
  { value: "staging", label: "Staging" },
  { value: "production", label: "Production" }
];

/** Which attempt the Checks panel shows: staging's, or the one run on production
 *  after a move. Offered only once a production attempt exists. */
export const EnvironmentSwitch = ({
  value,
  onChange
}: {
  value: BuildChecksEnvironment;
  onChange: (environment: BuildChecksEnvironment) => void;
}) => (
  <fieldset
    className='m-0 flex gap-1 border-0 p-0'
    aria-label='Environment'
    data-testid='build-checks-environment'
  >
    {ENVIRONMENTS.map((env) => (
      <Button
        key={env.value}
        size='sm'
        variant={env.value === value ? "secondary" : "ghost"}
        className='h-6 px-2 text-xs'
        aria-pressed={env.value === value}
        onClick={() => onChange(env.value)}
        data-testid={`build-checks-environment-${env.value}`}
      >
        {env.label}
      </Button>
    ))}
  </fieldset>
);
