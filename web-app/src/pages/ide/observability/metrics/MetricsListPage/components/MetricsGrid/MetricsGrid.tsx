import { Sparkles } from "lucide-react";
import { useCallback, useState } from "react";
import { useNavigate } from "react-router-dom";
import EmptyState from "@/components/ui/EmptyState";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle
} from "@/components/ui/shadcn/card";
import { Spinner } from "@/components/ui/shadcn/spinner";
import TablePagination from "@/components/ui/TablePagination";
import { useMetricsList } from "@/hooks/api/metrics";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import type { DaysValue, ViewMode } from "../../constants";
import { METRICS_PAGE_SIZE } from "../../constants";
import MetricCard from "./MetricCard";
import MetricListItem from "./MetricListItem";

interface MetricsGridProps {
  viewMode: ViewMode;
  daysFilter: DaysValue;
}

export default function MetricsGrid({ viewMode, daysFilter }: MetricsGridProps) {
  const navigate = useNavigate();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const [currentPage, setCurrentPage] = useState(1);
  const offset = (currentPage - 1) * METRICS_PAGE_SIZE;

  // Fetch paginated metrics list
  const {
    data: listData,
    isLoading,
    isFetching
  } = useMetricsList(daysFilter, METRICS_PAGE_SIZE, offset);

  const metrics = listData?.metrics || [];
  const maxCount = metrics[0]?.count || 1;
  const totalMetrics = listData?.total || 0;
  const totalPages = Math.ceil(totalMetrics / METRICS_PAGE_SIZE);

  const hasData = listData && listData?.total > 0;

  const handleMetricClick = useCallback(
    (metricName: string) => {
      navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.OBSERVABILITY.METRIC(metricName));
    },
    [navigate, orgSlug]
  );

  const renderContent = () => {
    if (isLoading || isFetching) {
      return (
        <div className='flex items-center justify-center py-12'>
          <Spinner className='size-8 text-muted-foreground' />
        </div>
      );
    }

    if (!hasData) {
      return (
        <div className='flex items-center justify-center py-12'>
          <EmptyState
            title='No metrics data'
            description='No metric usage data available for the selected time range'
          />
        </div>
      );
    }

    if (viewMode === "grid") {
      return (
        <div className='grid grid-cols-1 gap-4 sm:grid-cols-2 xl:grid-cols-3'>
          {metrics.map((metric, index) => (
            <MetricCard
              key={metric.name}
              metric={metric}
              rank={offset + index + 1}
              maxCount={maxCount}
              onClick={() => handleMetricClick(metric.name)}
            />
          ))}
        </div>
      );
    }

    return (
      <div className='space-y-2'>
        {metrics.map((metric, index) => (
          <MetricListItem
            key={metric.name}
            metric={metric}
            rank={offset + index + 1}
            maxCount={maxCount}
            onClick={() => handleMetricClick(metric.name)}
          />
        ))}
      </div>
    );
  };

  return (
    <Card className='bg-transparent shadow-none'>
      <CardHeader className='pb-3'>
        <div className='flex items-center justify-between'>
          <div className='flex items-center gap-2'>
            <Sparkles className='h-5 w-5 text-primary' />
            <CardTitle>Top Metrics</CardTitle>
          </div>
          <span className='text-muted-foreground text-sm'>{totalMetrics} metrics</span>
        </div>
        <CardDescription>Click any metric to explore detailed usage patterns</CardDescription>
      </CardHeader>
      <CardContent className='space-y-4'>
        {renderContent()}

        {/* Pagination Controls */}
        {!isLoading && !isFetching && (
          <TablePagination
            currentPage={currentPage}
            totalPages={totalPages}
            totalItems={totalMetrics}
            pageSize={METRICS_PAGE_SIZE}
            onPageChange={setCurrentPage}
            itemLabel='metrics'
          />
        )}
      </CardContent>
    </Card>
  );
}
