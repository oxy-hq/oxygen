import type { CustomApp } from "@/types/apps";
import { RetainedBuilds } from "../../AppDetail/components/BuildHistory/RetainedBuilds";
import { Environments } from "../../AppDetail/components/Environments";
import { Publishers } from "../../AppDetail/components/Publishers";
import { Releases } from "../../AppDetail/components/Releases";
import { AppLifecycle } from "../../AppLifecycle";
import { ConsolePanel } from "../../ConsolePanel";
import { PublishCiPanel } from "../PublishCiPanel";
import { Column, TwoColumns } from "./layout";

export const ReleasesPage = ({
  app,
  onOpenFunction
}: {
  app: CustomApp;
  onOpenFunction: (name: string) => void;
}) => (
  <div data-testid='apps-console-page-releases'>
    <TwoColumns>
      <ConsolePanel
        id='build'
        title='Live build & releases'
        info='What production serves and the moves that change it, then each release with whether it made things worse, then the retained builds a rollback can choose from.'
      >
        <div className='flex flex-col gap-3'>
          <AppLifecycle app={app} />
          <Releases app={app} onOpenFunction={onOpenFunction} />
          <RetainedBuilds appId={app.id} />
        </div>
      </ConsolePanel>
      <Column>
        <ConsolePanel
          id='environments'
          title='Environments'
          info='Production, staging, and the dev sandboxes oxyc creates, each with its build.'
        >
          <Environments appId={app.id} />
        </ConsolePanel>
        <PublishCiPanel app={app} />
        <ConsolePanel
          id='publishers'
          title='Trusted publishers'
          info='The GitHub workflows allowed to publish this app with OIDC, storing no long-lived secret.'
        >
          <Publishers appId={app.id} />
        </ConsolePanel>
      </Column>
    </TwoColumns>
  </div>
);
