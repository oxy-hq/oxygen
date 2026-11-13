import WorkspaceRepositoryNotice from "@/components/WorkspaceRepositoryNotice";

/**
 * Where the agentic setup wizard was. It was a run of the Builder, which wrote
 * the workspace's files; a workspace is set up in its repository now, and
 * provisioning (the org, the workspace row, its repository) is unchanged.
 */
export default function AgenticSetupPage() {
  return (
    <div className='flex flex-1 justify-center p-6' data-testid='onboarding-setup-in-repository'>
      <div className='flex w-full max-w-xl flex-col gap-2'>
        <h1 className='font-semibold text-lg'>Set up this workspace in its repository</h1>
        <WorkspaceRepositoryNotice />
      </div>
    </div>
  );
}
