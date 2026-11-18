import type { CustomApp } from "@/types/apps";
import { Secrets, SecretsBadge } from "../../AppDetail/components/Secrets";
import { ConsolePanel } from "../../ConsolePanel";
import type { Scope } from "./layout";

export const SecretsPage = ({
  app,
  environment,
  scope,
  onEnvironmentChange
}: {
  app: CustomApp;
  environment: string;
  scope: Scope;
  onEnvironmentChange: (next: string) => void;
}) => (
  <div data-testid='apps-console-page-secrets'>
    <ConsolePanel
      id='secrets'
      title='Secrets'
      scope={scope}
      info='The keys the build declares and the values stored for them. A value is never shown here until you reveal it, and each reveal is recorded.'
      actions={<SecretsBadge appId={app.id} environment={environment} />}
    >
      <Secrets appId={app.id} environment={environment} onEnvironmentChange={onEnvironmentChange} />
    </ConsolePanel>
  </div>
);
