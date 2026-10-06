import { CheckCircle2, Terminal, XCircle } from "lucide-react";
import type React from "react";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle
} from "@/components/ui/shadcn/card";
import { Spinner } from "@/components/ui/shadcn/spinner";

export type CliAuthStatus = "working" | "confirm" | "done" | "error";

interface Props {
  status: CliAuthStatus;
  title: React.ReactNode;
  description: React.ReactNode;
  /** Actions under the description. Omit for a state with nothing to do. */
  children?: React.ReactNode;
}

const StatusIcon: React.FC<{ status: CliAuthStatus }> = ({ status }) => {
  switch (status) {
    case "error":
      return <XCircle className='h-12 w-12 text-destructive' />;
    case "done":
      return <CheckCircle2 className='h-12 w-12 text-primary' />;
    case "confirm":
      return <Terminal className='h-12 w-12 text-muted-foreground' />;
    case "working":
      return <Spinner className='h-12 w-12' />;
  }
};

/** The one card `/cli-auth` shows, whichever flow is running. */
const CliAuthCard: React.FC<Props> = ({ status, title, description, children }) => (
  <div className='flex min-h-screen w-full items-center justify-center bg-background p-4'>
    <Card className='w-full max-w-md' data-testid='cli-auth-card' data-status={status}>
      <CardHeader className='text-center'>
        <div className='mb-4 flex justify-center'>
          <StatusIcon status={status} />
        </div>
        <CardTitle className='text-2xl'>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent>{children}</CardContent>
    </Card>
  </div>
);

export default CliAuthCard;
