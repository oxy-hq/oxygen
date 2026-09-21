import { ArrowLeft } from "lucide-react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { AccessPane } from "./components/OxyAccessPanes/AccessPane";

/**
 * The second escape hatch: **which orgs have locked Oxy staff out of a workspace.**
 *
 * This survived a deletion it was nearly caught by. It lived in the "Organizations" tab,
 * and this direction removes that tab — the accepted cost being that browsing *what apps
 * an org has* is gone, since the console and the palette answer it better. But that tab
 * carried a second, unrelated thing: `buildAccessOrgs` is the only place in the product
 * that answers "has anyone locked us out?", and staff reach being revoked is a security
 * signal, not a directory listing. Dropping it along with the tab would have been a real
 * capability regression hidden inside a redesign.
 *
 * So it is routed like the storage audit: reached by link, never a tab, because a tab is
 * how this surface grew four of them.
 *
 * **Known redundancy, deliberately left for a follow-up:** `AccessPane` also lists each
 * org's apps, which the switcher and console now cover. Splitting the lockdown half out
 * cleanly is a change to a tested model (`accessModel.test.ts`) and belongs in its own
 * commit rather than riding along with a layout rewrite.
 */
export default function AccessAudit() {
  return (
    <div className='flex h-[calc(100vh-3.5rem)] flex-col' data-testid='apps-access-audit'>
      <header className='flex h-10 shrink-0 items-center gap-2 border-b px-3'>
        <Button asChild variant='ghost' size='sm' className='h-7 gap-1.5 text-xs'>
          <Link to='/admin/apps' data-testid='apps-access-audit-back'>
            <ArrowLeft className='size-3.5' />
            Back to apps
          </Link>
        </Button>
        <span className='text-muted-foreground text-xs'>
          Oxy staff access — which orgs have locked us out of a workspace
        </span>
      </header>
      <div className='min-h-0 flex-1 overflow-auto'>
        <AccessPane />
      </div>
    </div>
  );
}
