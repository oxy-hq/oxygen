import { History, List as ListIcon, ShieldCheck, Sparkles } from "lucide-react";
import { useState } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle
} from "@/components/ui/shadcn/card";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/shadcn/tabs";
import TablePagination from "@/components/ui/TablePagination";
import { useExecutionSummary, useExecutions } from "@/hooks/api/useExecutionAnalytics";
import { EXECUTION_TYPES, type ExecutionType } from "../types";
import ExecutionCard from "./ExecutionCard";

const PAGE_SIZE = 10;

interface ExecutionListProps {
  projectId: string | undefined;
  days: number;
}

type FilterTab = "all" | "verified" | "generated";

export default function ExecutionList({ projectId, days }: ExecutionListProps) {
  const [activeTab, setActiveTab] = useState<FilterTab>("all");
  const [statusFilter, setStatusFilter] = useState<string>("all");
  const [typeFilter, setTypeFilter] = useState<string>("all");
  const [currentPage, setCurrentPage] = useState(1);

  // Build query params based on filters
  const getIsVerifiedFilter = () => {
    if (activeTab === "verified") return true;
    if (activeTab === "generated") return false;
    return undefined;
  };
  const isVerifiedFilter = getIsVerifiedFilter();
  const executionType = typeFilter !== "all" ? typeFilter : undefined;
  const statusParam = statusFilter !== "all" ? statusFilter : undefined;
  const offset = (currentPage - 1) * PAGE_SIZE;

  // Fetch executions with backend filters
  const { data, isLoading } = useExecutions(projectId, {
    days,
    limit: PAGE_SIZE,
    offset,
    executionType,
    isVerified: isVerifiedFilter,
    status: statusParam
  });

  // Fetch summary for counts
  const { data: summaryData } = useExecutionSummary(projectId, { days });

  const executions = data?.executions ?? [];
  const total = data?.total ?? 0;

  const summary = summaryData;
  const totalPages = Math.ceil(total / PAGE_SIZE);

  const counts = {
    all: (summary?.verifiedCount ?? 0) + (summary?.generatedCount ?? 0),
    verified: summary?.verifiedCount ?? 0,
    generated: summary?.generatedCount ?? 0
  };

  const availableTypes = Object.keys(EXECUTION_TYPES) as ExecutionType[];

  const handleTabChange = (v: string) => {
    setActiveTab(v as FilterTab);
    setCurrentPage(1);
  };

  const handleStatusFilterChange = (v: string) => {
    setStatusFilter(v);
    setCurrentPage(1);
  };

  const handleTypeFilterChange = (v: string) => {
    setTypeFilter(v);
    setCurrentPage(1);
  };

  if (isLoading) {
    return (
      <Card className='bg-transparent shadow-none'>
        <CardHeader>
          <div className='flex items-center gap-2'>
            <History className='h-5 w-5 text-primary' />
            <CardTitle>Recent Executions</CardTitle>
          </div>
          <CardDescription>Browse and filter execution history</CardDescription>
        </CardHeader>
        <CardContent>
          <div className='space-y-3'>
            {[1, 2, 3].map((i) => (
              <div key={i} className='h-40 animate-pulse bg-muted' />
            ))}
          </div>
        </CardContent>
      </Card>
    );
  }

  return (
    <Card className='bg-transparent shadow-none'>
      <CardHeader className='pb-0'>
        <div className='flex items-center gap-2'>
          <History className='h-5 w-5 text-primary' />
          <CardTitle>Recent Executions</CardTitle>
        </div>
        <CardDescription>Browse and filter execution history</CardDescription>
      </CardHeader>
      <CardContent className='pt-4'>
        <Tabs value={activeTab} onValueChange={handleTabChange}>
          <div className='mb-4 flex flex-wrap items-center justify-between gap-3'>
            <TabsList>
              <TabsTrigger value='all' className='flex items-center gap-2'>
                <ListIcon className='h-4 w-4' />
                All
                <span className='ml-1 bg-muted px-1.5 py-0.5 text-xs'>{counts.all}</span>
              </TabsTrigger>
              <TabsTrigger value='verified' className='flex items-center gap-2'>
                <ShieldCheck className='h-4 w-4' />
                Verified
                <span className='ml-1 bg-muted px-1.5 py-0.5 text-xs'>{counts.verified}</span>
              </TabsTrigger>
              <TabsTrigger value='generated' className='flex items-center gap-2'>
                <Sparkles className='h-4 w-4' />
                Generated
                <span className='ml-1 bg-muted px-1.5 py-0.5 text-xs'>{counts.generated}</span>
              </TabsTrigger>
            </TabsList>

            <div className='flex items-center gap-2'>
              {/* Type Filter */}
              <Select value={typeFilter} onValueChange={handleTypeFilterChange}>
                <SelectTrigger className='w-40'>
                  <SelectValue placeholder='Type' />
                </SelectTrigger>
                <SelectContent className={DESIGN_REFERENCE}>
                  <SelectItem className='cursor-pointer' value='all'>
                    All Types
                  </SelectItem>
                  {availableTypes.map((type) => (
                    <SelectItem className='cursor-pointer' key={type} value={type}>
                      {EXECUTION_TYPES[type].shortLabel}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>

              {/* Status Filter */}
              <Select value={statusFilter} onValueChange={handleStatusFilterChange}>
                <SelectTrigger className='w-32'>
                  <SelectValue placeholder='Status' />
                </SelectTrigger>
                <SelectContent className={DESIGN_REFERENCE}>
                  <SelectItem className='cursor-pointer' value='all'>
                    All Status
                  </SelectItem>
                  <SelectItem className='cursor-pointer' value='success'>
                    Success
                  </SelectItem>
                  <SelectItem className='cursor-pointer' value='error'>
                    Error
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>

          <TabsContent value={activeTab} className='mt-0'>
            {isLoading && (
              <div className='space-y-3'>
                {[1, 2, 3].map((i) => (
                  <div key={i} className='h-40 animate-pulse bg-muted' />
                ))}
              </div>
            )}
            {!isLoading && executions.length === 0 && (
              <div className='py-8 text-center text-muted-foreground'>
                No executions found matching the current filters
              </div>
            )}
            {!isLoading && executions.length > 0 && (
              <div className='space-y-3'>
                {executions.map((execution) => (
                  <ExecutionCard
                    key={`${execution.traceId}-${execution.spanId}`}
                    execution={execution}
                  />
                ))}
              </div>
            )}

            {/* Pagination */}
            {!isLoading && (
              <TablePagination
                currentPage={currentPage}
                totalPages={totalPages}
                totalItems={total}
                pageSize={PAGE_SIZE}
                onPageChange={setCurrentPage}
                itemLabel='executions'
              />
            )}
          </TabsContent>
        </Tabs>
      </CardContent>
    </Card>
  );
}
