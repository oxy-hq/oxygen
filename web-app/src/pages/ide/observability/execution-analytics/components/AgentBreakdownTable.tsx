import { Users } from "lucide-react";
import { useNavigate } from "react-router-dom";
import { Badge } from "@/components/ui/shadcn/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle
} from "@/components/ui/shadcn/card";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from "@/components/ui/shadcn/table";
import { useExecutionAgentStats } from "@/hooks/api/useExecutionAnalytics";
import { encodeBase64 } from "@/libs/encoding";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import { EXECUTION_TYPES, type ExecutionType } from "../types";

interface AgentBreakdownTableProps {
  projectId: string | undefined;
  days: number;
  limit?: number;
  onAgentClick?: (agentRef: string) => void;
}

function getExecutionTypeLabel(type: string): string {
  if (type === "none") return "None";
  const typeInfo = EXECUTION_TYPES[type as ExecutionType];
  return typeInfo?.shortLabel ?? type;
}

function getSuccessRateBadgeColor(successRate: number): string {
  if (successRate > 95) {
    return "bg-success/10 text-success";
  }
  if (successRate > 80) {
    return "bg-warning/10 text-warning";
  }
  return "bg-destructive/10 text-destructive";
}

function DistributionBar({ verified, generated }: { verified: number; generated: number }) {
  return (
    <div className='flex h-2.5 w-28 overflow-hidden rounded-full bg-muted'>
      <div
        className='bg-success transition-all'
        style={{ width: `${verified}%` }}
        title={`Verified: ${verified.toFixed(1)}%`}
      />
      <div
        className='bg-vis-orange transition-all'
        style={{ width: `${generated}%` }}
        title={`Generated: ${generated.toFixed(1)}%`}
      />
    </div>
  );
}

export default function AgentBreakdownTable({
  projectId,
  days,
  limit = 10
}: AgentBreakdownTableProps) {
  const navigate = useNavigate();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const { data: agents = [], isLoading } = useExecutionAgentStats(projectId, {
    days,
    limit
  });
  if (isLoading) {
    return (
      <Card className='bg-transparent shadow-none'>
        <CardHeader>
          <div className='flex items-center gap-2'>
            <Users className='h-5 w-5 text-primary' />
            <CardTitle>Agent Breakdown</CardTitle>
          </div>
          <CardDescription>Execution methods by agent</CardDescription>
        </CardHeader>
        <CardContent>
          <div className='animate-pulse space-y-2'>
            {[1, 2, 3].map((i) => (
              <div key={i} className='h-10 rounded bg-muted' />
            ))}
          </div>
        </CardContent>
      </Card>
    );
  }

  if (agents.length === 0) {
    return (
      <Card className='bg-transparent shadow-none'>
        <CardHeader>
          <div className='flex items-center gap-2'>
            <Users className='h-5 w-5 text-primary' />
            <CardTitle>Agent Breakdown</CardTitle>
          </div>
          <CardDescription>Execution methods by agent</CardDescription>
        </CardHeader>
        <CardContent>
          <p className='py-4 text-center text-muted-foreground text-sm'>No agent data available</p>
        </CardContent>
      </Card>
    );
  }

  return (
    <Card className='bg-transparent shadow-none'>
      <CardHeader>
        <div className='flex items-center gap-2'>
          <Users className='h-5 w-5 text-primary' />
          <CardTitle>Agent Breakdown</CardTitle>
        </div>
        <CardDescription>Execution methods by agent</CardDescription>
      </CardHeader>
      <CardContent>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Agent</TableHead>
              <TableHead className='text-right'>Total</TableHead>
              <TableHead className='text-right text-success'>Verified</TableHead>
              <TableHead className='text-right text-vis-orange'>Generated</TableHead>
              <TableHead>Distribution</TableHead>
              <TableHead>Most Executed</TableHead>
              <TableHead className='text-right'>Success Rate</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {agents.map((agent) => (
              <TableRow key={agent.agentRef} className='hover:bg-muted/50'>
                <TableCell className='font-medium'>
                  <button
                    onClick={() => {
                      const pathb64 = encodeBase64(agent.agentRef);
                      navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.FILES.FILE(pathb64));
                    }}
                    className='text-left underline-offset-4 transition-colors hover:text-primary hover:underline'
                    title={agent.agentRef}
                  >
                    {agent.agentRef.split("/").pop()}
                  </button>
                </TableCell>
                <TableCell className='text-right font-medium'>{agent.totalExecutions}</TableCell>
                <TableCell className='text-right font-medium text-success'>
                  {agent.verifiedCount}
                </TableCell>
                <TableCell className='text-right font-medium text-vis-orange'>
                  {agent.generatedCount}
                </TableCell>
                <TableCell>
                  <DistributionBar
                    verified={agent.verifiedPercent}
                    generated={100 - agent.verifiedPercent}
                  />
                </TableCell>
                <TableCell>{getExecutionTypeLabel(agent.mostExecutedType)}</TableCell>
                <TableCell className='text-right'>
                  <Badge
                    variant='secondary'
                    className={cn(getSuccessRateBadgeColor(agent.successRate))}
                  >
                    {agent.successRate.toFixed(1)}%
                  </Badge>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </CardContent>
    </Card>
  );
}
