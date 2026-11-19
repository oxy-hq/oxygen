import { ArrowRight } from "lucide-react";
import useSettingsDialog, { type SettingsSection } from "@/stores/useSettingsDialog";
import type { ApiTokensTab } from "../../../../tabs";
import { useCanOpenSection } from "../../../../useVisibleNavGroups";

/** Where one answer leads: a tab of this page, or another section. */
type Destination =
  | { kind: "tab"; tab: ApiTokensTab }
  | { kind: "section"; section: SettingsSection; tab?: string };

interface Row {
  need: string;
  answer: string;
  to: Destination;
  /** Read in the guide's own words when the viewer cannot follow the link. */
  unreachable: string;
  testId: string;
}

const SECRETS: SettingsSection = "organization.secrets";

/**
 * Each row answers "I need a token for X" with the kind to use, and goes to where that kind is
 * made. Every answer here is a decision already made in the token design
 * (`internal-docs/2026-09-30-api-tokens-design.md` §3.1): a personal token is its owner, a
 * service account outlives whoever set it up, and GitHub Actions needs no stored secret at all.
 * A custom app that manages kiosk devices or uploads source reports needs no token at all: an
 * admin approves it under Apps and it calls `ctx.oxygen`. One that calls any other Oxygen API
 * keeps a service account token as one of its secrets, so that row leads to Secrets, where the
 * token can be created straight into the secret.
 */
const ROWS: readonly Row[] = [
  {
    need: "oxyc, or a script that acts as you",
    answer: "Personal access token",
    to: { kind: "tab", tab: "personal" },
    unreachable: "Personal access token",
    testId: "personal"
  },
  {
    need: "A job or integration that keeps working after you leave",
    answer: "Service account token",
    to: { kind: "tab", tab: "service_accounts" },
    unreachable: "Service account token, from an organization admin",
    testId: "service-account"
  },
  {
    need: "GitHub Actions",
    answer: "Trusted access on a service account: no token is stored",
    to: { kind: "tab", tab: "service_accounts" },
    unreachable: "Trusted access, set up by an organization admin",
    testId: "trusted-access"
  },
  {
    need: "A custom app that manages kiosk devices or uploads source reports",
    answer: "Approve the app under Apps: no token is stored",
    to: { kind: "tab", tab: "apps" },
    unreachable: "An approval under Apps, from an organization admin",
    testId: "app-approval"
  },
  {
    need: "A custom app that calls any other Oxygen API",
    answer: "Create API token on the app's secret",
    to: { kind: "section", section: SECRETS, tab: "app" },
    unreachable: "A service account token in the app's secret, from an organization admin",
    testId: "custom-app"
  },
  {
    need: "Another service's key, for your apps",
    answer: "Secrets",
    to: { kind: "section", section: SECRETS },
    unreachable: "A secret, set by an organization admin",
    testId: "secret"
  }
];

interface TokenGuideProps {
  /** The tabs of this page the viewer can see. */
  visibleTabs: readonly ApiTokensTab[];
}

/**
 * "Which token do I need?": the top of Organization → API tokens. It exists because the kinds of
 * credential used to live in three places (Account, Organization → API access, Workspace →
 * Secrets) and a person fixing a custom app's key had to visit all three to work out which one
 * they were holding. An answer the viewer cannot act on is still said, in plain words naming who
 * can, rather than hidden.
 */
export function TokenGuide({ visibleTabs }: TokenGuideProps) {
  const setTab = useSettingsDialog((s) => s.setTab);
  const open = useSettingsDialog((s) => s.open);
  const canOpenSecrets = useCanOpenSection(SECRETS);

  const follow = (to: Destination): (() => void) | null => {
    if (to.kind === "tab") return visibleTabs.includes(to.tab) ? () => setTab(to.tab) : null;
    if (to.section === SECRETS && !canOpenSecrets) return null;
    return () => open(to.section, to.tab ?? null);
  };

  return (
    <section
      className='rounded-md border'
      aria-labelledby='api-tokens-guide-title'
      data-testid='api-tokens-guide'
    >
      <h4 id='api-tokens-guide-title' className='border-b px-3 py-2 font-medium text-xs'>
        Which token do I need?
      </h4>
      <dl className='divide-y text-xs'>
        {ROWS.map((row) => {
          const onClick = follow(row.to);
          return (
            <div
              key={row.testId}
              className='flex flex-col gap-0.5 px-3 py-2 sm:flex-row sm:items-center sm:gap-4'
              data-testid={`api-tokens-guide-${row.testId}`}
            >
              <dt className='text-muted-foreground sm:w-1/2 sm:shrink-0'>{row.need}</dt>
              <dd className='min-w-0'>
                {onClick ? (
                  <button
                    type='button'
                    onClick={onClick}
                    className='inline-flex items-center gap-1 text-left font-medium underline-offset-2 hover:underline'
                  >
                    {row.answer}
                    <ArrowRight className='size-3 shrink-0' aria-hidden />
                  </button>
                ) : (
                  <span>{row.unreachable}</span>
                )}
              </dd>
            </div>
          );
        })}
      </dl>
    </section>
  );
}
