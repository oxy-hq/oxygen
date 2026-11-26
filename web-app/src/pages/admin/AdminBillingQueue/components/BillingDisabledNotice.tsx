import { Flag } from "lucide-react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { Card, CardContent } from "@/components/ui/shadcn/card";

export function BillingDisabledNotice() {
  return (
    <Card>
      <CardContent className='flex flex-col items-center gap-3 py-12 text-center'>
        <Flag className='size-8 text-muted-foreground' />
        <div className='space-y-1'>
          <p className='font-medium'>Billing is disabled</p>
          <p className='max-w-md text-muted-foreground text-xs'>
            The <code className='bg-muted px-1 py-0.5 text-xs'>billing</code> feature flag is off,
            so subscription provisioning, the paywall, and Stripe integration are all skipped.
            Enable the flag to use the queue.
          </p>
        </div>
        <Button asChild variant='outline' size='sm'>
          <Link to='/admin/feature-flags'>Open Feature flags</Link>
        </Button>
      </CardContent>
    </Card>
  );
}
