import { ArrowUpRight, Gauge } from "lucide-react";
import { Link } from "react-router-dom";
import ROUTES from "@/libs/utils/routes";

/**
 * Where cost and app performance went.
 *
 * The tenant directory used to carry the whole LLM cost command centre. It now
 * carries a pointer, because the full answer is per org and this page is a
 * cross-tenant triage hub — and because a demoted surface with no inbound link
 * is an unreachable one, which is a trap this console has already fallen into
 * once (`/admin/publish-tokens`, reachable as a module and not as a surface).
 */
export const ObservabilityLink = () => (
  <Link
    to={ROUTES.ADMIN.OBSERVABILITY}
    data-testid='admin-tenants-observability-link'
    className='flex items-center gap-3 rounded-lg border border-border/60 bg-card p-4 transition-colors hover:border-foreground/20 hover:bg-card/80'
  >
    <Gauge className='size-4 shrink-0 text-muted-foreground' />
    <span className='min-w-0 flex-1 text-xs'>
      <span className='block font-medium'>Cost &amp; performance</span>
      <span className='block text-muted-foreground'>
        What each organization costs us, and how its custom apps are performing.
      </span>
    </span>
    <ArrowUpRight className='size-3.5 shrink-0 text-muted-foreground' />
  </Link>
);
