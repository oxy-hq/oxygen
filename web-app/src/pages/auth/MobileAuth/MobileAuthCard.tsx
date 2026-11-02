import { CheckCircle2, Smartphone, XCircle } from "lucide-react";
import type React from "react";
import { Card, CardContent, CardDescription, CardHeader } from "@/components/ui/shadcn/card";
import { Spinner } from "@/components/ui/shadcn/spinner";

type MobileAuthStatus = "working" | "confirm" | "done" | "error";

interface Props {
  status: MobileAuthStatus;
  title: React.ReactNode;
  description: React.ReactNode;
  /** The signed-in person's initial, shown paired with the phone while they decide. */
  initial?: string;
  children?: React.ReactNode;
}

/** What sits across from the phone: the account being paired, or how the pairing went. */
const Counterpart: React.FC<{ status: MobileAuthStatus; initial?: string }> = ({
  status,
  initial
}) => {
  switch (status) {
    case "confirm":
      return (
        <span className='t-button flex size-10 items-center justify-center rounded-full bg-primary text-primary-foreground'>
          {initial ?? "?"}
        </span>
      );
    case "working":
      return <Spinner className='size-6' />;
    case "done":
      return <CheckCircle2 className='size-7 text-primary' />;
    case "error":
      return <XCircle className='size-7 text-destructive' />;
  }
};

/**
 * The one card `/mobile-auth` shows. It opens on the phone itself, inside the app's sign-in
 * sheet, so it is laid out for a narrow screen first: left-aligned, buttons a thumb's width.
 */
const MobileAuthCard: React.FC<Props> = ({ status, title, description, initial, children }) => (
  <div className='flex min-h-dvh w-full items-start justify-center bg-background px-4 py-10 sm:items-center'>
    <Card className='w-full max-w-sm' data-testid='mobile-auth-card' data-status={status}>
      <CardHeader className='space-y-3'>
        <div className='flex items-center gap-2' aria-hidden='true'>
          <span className='flex size-10 items-center justify-center rounded-xl bg-muted text-foreground'>
            <Smartphone className='size-5' />
          </span>
          <span className='w-6 border-muted-foreground/40 border-t border-dashed' />
          <Counterpart status={status} initial={initial} />
        </div>
        <h1 className='t-h2 text-balance'>{title}</h1>
        <CardDescription className='t-body'>{description}</CardDescription>
      </CardHeader>
      {children && <CardContent>{children}</CardContent>}
    </Card>
  </div>
);

export default MobileAuthCard;
