import { ArrowLeft } from "lucide-react";
import { useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useMetricDetail } from "@/hooks/api/metrics";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import DetailHeader from "./components/DetailHeader";
import DetailStatsRow from "./components/DetailStatsRow";
import RecentUsageSection from "./components/RecentUsage/RecentUsageSection";
import RelatedMetrics from "./components/RelatedMetrics";
import TrendChart from "./components/TrendChart";
import type { DaysValue } from "./constants";

export default function MetricsDetailPage() {
  const { metricName } = useParams<{ metricName: string }>();
  const navigate = useNavigate();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const [daysFilter, setDaysFilter] = useState<DaysValue>(30);

  const decodedMetricName = metricName ? decodeURIComponent(metricName) : "";

  const {
    data: detailData,
    isLoading,
    error: detailError
  } = useMetricDetail(decodedMetricName, daysFilter);

  const handleBack = () => {
    navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.OBSERVABILITY.METRICS);
  };

  const handleRelatedMetricClick = (relatedMetric: string) => {
    navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.OBSERVABILITY.METRIC(relatedMetric));
  };

  const handleTraceClick = (traceId: string) => {
    navigate(ROUTES.ORG(orgSlug).WORKSPACE().IDE.OBSERVABILITY.TRACE(traceId));
  };

  if (isLoading) {
    return (
      <div className='flex h-full flex-col'>
        <div className='flex items-center border-b p-4'>
          <Button variant='ghost' size='icon' onClick={handleBack} className='hover:bg-muted'>
            <ArrowLeft className='h-4 w-4' />
          </Button>
          <h1 className='ml-4 font-semibold text-xl'>{decodedMetricName}</h1>
        </div>
        <div className='flex flex-1 items-center justify-center'>
          <Spinner className='size-8 text-muted-foreground' />
        </div>
      </div>
    );
  }

  if (detailError || !detailData) {
    return (
      <div className='flex h-full flex-col'>
        <div className='flex items-center border-b p-4'>
          <Button variant='ghost' size='icon' onClick={handleBack} className='hover:bg-muted'>
            <ArrowLeft className='h-4 w-4' />
          </Button>
          <h1 className='ml-4 font-semibold text-xl'>{decodedMetricName}</h1>
        </div>
        <div className='flex flex-1 items-center justify-center p-4'>
          <ErrorAlert message='Failed to load metric details' />
        </div>
      </div>
    );
  }

  return (
    <div className='flex h-full flex-col'>
      <div className='flex min-h-0 flex-1 flex-col'>
        <DetailHeader
          metricName={decodedMetricName}
          detailData={detailData}
          daysFilter={daysFilter}
          onBack={handleBack}
          onDaysFilterChange={setDaysFilter}
        />

        {/* Content */}
        <div className='min-h-0 flex-1 overflow-auto p-6'>
          <div className='mx-auto max-w-6xl space-y-6'>
            <DetailStatsRow detailData={detailData} />

            <div className='grid grid-cols-1 gap-6 lg:grid-cols-3'>
              <TrendChart detailData={detailData} daysFilter={daysFilter} />
              <RelatedMetrics detailData={detailData} onMetricClick={handleRelatedMetricClick} />
            </div>

            <RecentUsageSection
              detailData={detailData}
              metricName={decodedMetricName}
              onTraceClick={handleTraceClick}
            />
          </div>
        </div>
      </div>
    </div>
  );
}
