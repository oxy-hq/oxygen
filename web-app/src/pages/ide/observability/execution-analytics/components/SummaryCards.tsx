import { Activity, CheckCircle, ShieldCheck, Sparkles, TrendingUp } from "lucide-react";
import { Card, CardContent } from "@/components/ui/shadcn/card";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { EXECUTION_TYPES, type ExecutionSummary, type ExecutionType } from "../types";

interface SummaryCardsProps {
  summary: ExecutionSummary;
  isLoading: boolean;
}

function formatNumber(num: number): string {
  if (num >= 1000000) return `${(num / 1000000).toFixed(1)}M`;
  if (num >= 1000) return `${(num / 1000).toFixed(1)}K`;
  return num.toString();
}

function getExecutionTypeLabel(type: string): string {
  if (type === "none") return "None";
  const typeInfo = EXECUTION_TYPES[type as ExecutionType];
  return typeInfo?.label ?? type;
}

interface StatsCardProps {
  title: string;
  value: string | number;
  subtitle: string;
  icon: React.ReactNode;
  isLoading?: boolean;
}

function StatsCard({ title, value, subtitle, icon, isLoading }: StatsCardProps) {
  return (
    <Card className='overflow-hidden bg-transparent shadow-none'>
      <CardContent className='p-4'>
        <div className='flex items-start justify-between'>
          <div className='min-w-0 space-y-1'>
            <div className='flex items-center gap-2'>
              <p className='font-medium text-muted-foreground text-xs'>{title}</p>
              {isLoading && <Spinner className='size-3 text-muted-foreground' />}
            </div>
            <p className='truncate font-bold text-2xl tracking-tight'>{value}</p>
            <p className='text-muted-foreground text-xs'>{subtitle}</p>
          </div>
          <div className='bg-primary/10 p-2 text-primary'>{icon}</div>
        </div>
      </CardContent>
    </Card>
  );
}

export default function SummaryCards({ summary, isLoading }: SummaryCardsProps) {
  return (
    <div className='grid grid-cols-2 gap-4 md:grid-cols-3 lg:grid-cols-5'>
      <StatsCard
        title='Total Executions'
        value={formatNumber(summary.totalExecutions)}
        subtitle='queries & automations'
        icon={<Activity className='h-5 w-5' />}
        isLoading={isLoading}
      />
      <StatsCard
        title='Verified'
        value={`${summary.verifiedPercent.toFixed(1)}%`}
        subtitle={`${formatNumber(summary.verifiedCount)} executions`}
        icon={<ShieldCheck className='h-5 w-5' />}
        isLoading={isLoading}
      />
      <StatsCard
        title='Generated'
        value={`${summary.generatedPercent.toFixed(1)}%`}
        subtitle={`${formatNumber(summary.generatedCount)} executions`}
        icon={<Sparkles className='h-5 w-5' />}
        isLoading={isLoading}
      />
      <StatsCard
        title='Most Executed'
        value={getExecutionTypeLabel(summary.mostExecutedType)}
        subtitle='most common type'
        icon={<TrendingUp className='h-5 w-5' />}
        isLoading={isLoading}
      />
      <StatsCard
        title='Success Rate'
        value={`${summary.successRateVerified.toFixed(1)}%`}
        subtitle={`verified (${summary.successRateGenerated.toFixed(1)}% generated)`}
        icon={<CheckCircle className='h-5 w-5' />}
        isLoading={isLoading}
      />
    </div>
  );
}
