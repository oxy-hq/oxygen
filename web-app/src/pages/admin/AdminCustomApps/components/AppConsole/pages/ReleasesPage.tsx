import { useLocation, useSearchParams } from "react-router-dom";
import type { CustomApp } from "@/types/apps";
import { RetainedBuilds } from "../../AppDetail/components/BuildHistory/RetainedBuilds";
import { Environments } from "../../AppDetail/components/Environments";
import { Publishers } from "../../AppDetail/components/Publishers";
import { Releases } from "../../AppDetail/components/Releases";
import { ConsolePanel } from "../../ConsolePanel";
import { BuildChecksPanel } from "../BuildChecks";
import { ConsoleLifecycle } from "../ConsoleLifecycle";
import {
  CHECKS_PARAM,
  checksEnvironment,
  withChecksBuild,
  withChecksEnvironment
} from "../checksParams";
import { PublishCiPanel } from "../PublishCiPanel";
import { Column, TwoColumns } from "./layout";

export const ReleasesPage = ({
  app,
  onOpenFunction
}: {
  app: CustomApp;
  onOpenFunction: (name: string) => void;
}) => {
  const { search } = useLocation();
  const [params, setParams] = useSearchParams();
  // `?checks=<build>` and `?checks_env=production`: which checks the panel shows.
  const showChecks = (build: string | null) => setParams((prev) => withChecksBuild(prev, build));

  return (
    <div data-testid='apps-console-page-releases'>
      <TwoColumns>
        <ConsolePanel
          id='build'
          title='Live build & releases'
          info='What production serves and the moves that change it, then each release with whether it made things worse, then the retained builds a rollback can choose from.'
        >
          <div className='flex flex-col gap-3'>
            <ConsoleLifecycle app={app} search={search} />
            <Releases app={app} onOpenFunction={onOpenFunction} />
            <RetainedBuilds appId={app.id} onOpenChecks={(b) => showChecks(b.build_id)} />
          </div>
        </ConsolePanel>
        <Column>
          <BuildChecksPanel
            appId={app.id}
            requested={params.get(CHECKS_PARAM)}
            onShowStaging={() => showChecks(null)}
            promotionChecks={app.promotion_checks}
            environment={checksEnvironment(params)}
            onEnvironment={(next) => setParams((prev) => withChecksEnvironment(prev, next))}
          />
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
};
