import type { CustomApp } from "@/types/apps";
import { AppStoragePanel } from "../AppStoragePanel";
import { AppDataPanel } from "../DataBrowser";
import { type Scope, TwoColumns } from "./layout";

export const DataPage = ({
  app,
  environment,
  scope
}: {
  app: CustomApp;
  environment: string;
  scope: Scope;
}) => (
  <div data-testid='apps-console-page-data'>
    <TwoColumns>
      <AppDataPanel app={app} environment={environment} scope={scope} />
      <AppStoragePanel app={app} />
    </TwoColumns>
  </div>
);
