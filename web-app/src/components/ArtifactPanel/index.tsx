import { useQueries } from "@tanstack/react-query";
import { X } from "lucide-react";
import { useCallback } from "react";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { Spinner } from "@/components/ui/shadcn/spinner";
import queryKeys from "@/hooks/api/queryKey";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { ArtifactService } from "@/services/api";
import type { Artifact } from "@/types/artifact";
import { Panel } from "../ui/panel";
import { Button } from "../ui/shadcn/button";
import AgentArtifactPanel from "./ArtifactsContent/agent";
import AutomationArtifactPanel from "./ArtifactsContent/automation";
import LookerQueryArtifactPanel from "./ArtifactsContent/looker-query";
import OmniQueryArtifactPanel from "./ArtifactsContent/omni-query";
import SandboxArtifactPanel from "./ArtifactsContent/sandbox-app";
import SemanticQueryArtifactPanel from "./ArtifactsContent/semantic-query";
import SqlArtifactPanel from "./ArtifactsContent/sql";
import Header from "./Header";

type Props = {
  selectedArtifactIds: string[];
  artifactStreamingData: { [key: string]: Artifact };
  onClose: () => void;
  setSelectedArtifactIds: React.Dispatch<React.SetStateAction<string[]>>;
  wrapperClassName?: string;
};

const ArtifactPanel = ({
  selectedArtifactIds,
  artifactStreamingData,
  wrapperClassName,
  onClose,
  setSelectedArtifactIds
}: Props) => {
  const onArtifactClick = useCallback(
    (id: string) => {
      setSelectedArtifactIds((prev) => [...prev, id]);
    },
    [setSelectedArtifactIds]
  );
  const { project } = useCurrentProjectBranch();
  const artifactQueries = useQueries({
    queries: selectedArtifactIds.map((id) => ({
      queryKey: queryKeys.artifact.get(project.id, id),
      queryFn: () => ArtifactService.getArtifact(project.id, id)
    }))
  });

  const isLoading = artifactQueries.some((query) => query.isLoading);
  const hasError = artifactQueries.some((query) => query.isError);

  const artifactAPIData: { [key: string]: Artifact } = artifactQueries
    .map((result) => result.data)
    .reduce(
      (acc, artifact) => {
        if (artifact) {
          acc[artifact.id] = artifact;
        }
        return acc;
      },
      {} as { [key: string]: Artifact }
    );
  const artifactData = {
    ...artifactStreamingData,
    ...artifactAPIData
  };

  const currentArtifactId = selectedArtifactIds[selectedArtifactIds.length - 1];
  const currentArtifact = artifactData[currentArtifactId];

  const isCurrentArtifactLoading =
    isLoading && !currentArtifact && !artifactStreamingData[currentArtifactId];

  if (isCurrentArtifactLoading) {
    return (
      <Panel>
        <div className='flex w-full justify-end px-4 py-2'>
          <Button variant='outline' size='icon' onClick={onClose}>
            <X />
          </Button>
        </div>

        <div className='flex flex-1 flex-col items-center justify-center space-y-4 text-muted-foreground'>
          <Spinner />
        </div>
      </Panel>
    );
  }

  if (hasError && !currentArtifact) {
    return (
      <Panel>
        <div className='flex w-full justify-end px-4 py-2'>
          <Button variant='outline' size='icon' onClick={onClose}>
            <X />
          </Button>
        </div>

        <div className='flex flex-1 flex-col items-center justify-center gap-4 p-4'>
          <ErrorAlert message='Unable to load the selected artifact. Please check your connection or try again later.' />
          <Button
            variant='outline'
            onClick={() =>
              artifactQueries.forEach((query) => {
                // refetch settles into the query's error state; it does not reject.
                void query.refetch();
              })
            }
          >
            Retry
          </Button>
        </div>
      </Panel>
    );
  }

  if (!currentArtifact) {
    return null;
  }

  const renderContent = () => {
    if (currentArtifact.kind === "execute_sql") {
      return <SqlArtifactPanel artifact={currentArtifact} />;
    }

    if (currentArtifact.kind === "semantic_query") {
      return <SemanticQueryArtifactPanel artifact={currentArtifact} />;
    }

    if (currentArtifact.kind === "omni_query") {
      return <OmniQueryArtifactPanel artifact={currentArtifact} />;
    }

    if (currentArtifact.kind === "looker_query") {
      return <LookerQueryArtifactPanel artifact={currentArtifact} />;
    }

    if (currentArtifact.kind === "agent") {
      return (
        <div className='h-full overflow-y-auto p-4'>
          <AgentArtifactPanel artifact={currentArtifact} onArtifactClick={onArtifactClick} />
        </div>
      );
    }

    if (currentArtifact.kind === "workflow") {
      return (
        <AutomationArtifactPanel onArtifactClick={onArtifactClick} artifact={currentArtifact} />
      );
    }

    if (currentArtifact.kind === "sandbox_app") {
      return <SandboxArtifactPanel artifact={currentArtifact} />;
    }

    return <div className='artifact-unknown'>Unsupported artifact type</div>;
  };

  const handleClose = () => {
    setSelectedArtifactIds([]);
    onClose();
  };

  return (
    <Panel className={wrapperClassName}>
      <Header
        currentArtifact={currentArtifact}
        artifactData={artifactData}
        setSelectedArtifactIds={setSelectedArtifactIds}
        selectedArtifactIds={selectedArtifactIds}
        onClose={handleClose}
      />
      <div className='min-h-0 flex-1'>{renderContent()}</div>
    </Panel>
  );
};

export default ArtifactPanel;
