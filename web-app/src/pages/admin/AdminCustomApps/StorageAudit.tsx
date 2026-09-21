import { ArrowLeft } from "lucide-react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import StorageTab from "./components/StorageTab";

/**
 * The one list view this direction keeps — and the one place it admits a list is the
 * right shape.
 *
 * "Which apps have no retention rule?" is a genuine cross-fleet question. The fleet list
 * carries a Storage column and totals, but not retention, growth, or the per-object
 * browser — those want a surface of their own rather than four more columns. So this
 * escape hatch is designed rather than
 * hand-waved: sortable, with a filter that actually partitions, and rows that resolve
 * **back into** the console instead of being somewhere you live.
 *
 * What makes it an escape hatch rather than the landing is where it is reached from. It
 * has no tab and no rail entry; you get here from a link at the foot of an app's console
 * or from its storage panel. Give it a tab and it quietly becomes the fleet page this
 * direction deleted — which is exactly how the surface grew four of them.
 *
 * The body is `StorageTab` unchanged. It was already the densest, most useful thing on
 * the old surface — stats, a trend, a ranked table, a per-app browser — and its problem
 * was never its content; it was being a top-level sibling of "the apps".
 */
export default function StorageAudit() {
  return (
    <div className='flex h-[calc(100vh-3.5rem)] flex-col' data-testid='apps-storage-audit'>
      <header className='flex h-10 shrink-0 items-center gap-2 border-b px-3'>
        <Button asChild variant='ghost' size='sm' className='h-7 gap-1.5 text-xs'>
          <Link to='/admin/apps' data-testid='apps-storage-audit-back'>
            <ArrowLeft className='size-3.5' />
            Back to apps
          </Link>
        </Button>
        <span className='text-muted-foreground text-xs'>
          Storage &amp; retention across every custom app
        </span>
      </header>
      <div className='min-h-0 flex-1 overflow-auto'>
        <StorageTab />
      </div>
    </div>
  );
}
