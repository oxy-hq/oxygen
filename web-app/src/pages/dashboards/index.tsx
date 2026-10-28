import { LayoutDashboard } from "lucide-react";
import { Link } from "react-router-dom";
import PageHeader from "@/components/PageHeader";
import { Card, CardHeader, CardTitle } from "@/components/ui/shadcn/card";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useApps from "@/hooks/api/apps/useApps";
import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { encodeBase64 } from "@/libs/encoding";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import { appDisplayLabel } from "@/utils/appLabel";

/** Published .app.yml Data Apps ("Dashboards"). Lives at /apps. */
export default function DashboardsPage() {
  const appsFirst = useAppsFirstExperience();
  const { data: apps, isPending } = useApps({ publishedOnly: true });
  const { project } = useCurrentProjectBranch();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const ws = ROUTES.ORG(orgSlug).WORKSPACE(project.id);

  return (
    <div className='flex h-full w-full flex-col'>
      <PageHeader />
      <div className='flex flex-1 flex-col gap-4 overflow-auto p-6'>
        <div>
          <h1 className='font-semibold text-2xl'>Dashboards</h1>
          <p className='text-muted-foreground text-sm'>
            {appsFirst ? "Published data apps." : "Published data apps in this workspace."}
          </p>
        </div>
        {isPending && (
          <div className='flex items-center justify-center py-12'>
            <Spinner />
          </div>
        )}
        {!isPending && (!apps || apps.length === 0) && (
          <p className='py-12 text-center text-muted-foreground'>No published dashboards yet.</p>
        )}
        {!isPending && apps && apps.length > 0 && (
          <div className='grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3'>
            {apps.map((app) => (
              <Link key={app.path} to={ws.APP(encodeBase64(app.path))}>
                <Card className='transition-colors hover:border-primary'>
                  <CardHeader>
                    <CardTitle className='flex items-center gap-2'>
                      <LayoutDashboard className='h-4 w-4 text-primary' />
                      {appDisplayLabel(app)}
                    </CardTitle>
                  </CardHeader>
                </Card>
              </Link>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
