import { ArrowLeft, ArrowRight } from "lucide-react";
import type React from "react";
import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { Textarea } from "@/components/ui/shadcn/textarea";
import useDatabases from "@/hooks/api/databases/useDatabases";
import useCreateFile from "@/hooks/api/files/useCreateFile";
import useSaveFile from "@/hooks/api/files/useSaveFile";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { apiErrorMessage } from "@/libs/apiError";
import { encodeBase64 } from "@/libs/encoding";
import { cn } from "@/libs/shadcn/utils";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import { buildPipelineScaffold, SOURCE_OPTIONS, WRITABLE_DESTINATION_DB_TYPES } from "../scaffold";
import ConnectorCard from "./ConnectorCard";
import ClickHouseCredentialsForm from "./credentials/clickhouse/ClickHouseCredentialsForm";
import { useClickHouseCredentials } from "./credentials/clickhouse/useClickHouseCredentials";
import QuickBooksCredentialsForm from "./credentials/quickbooks/QuickBooksCredentialsForm";
import { useQuickBooksCredentials } from "./credentials/quickbooks/useQuickBooksCredentials";
import SpApiCredentialsForm from "./credentials/spApi/SpApiCredentialsForm";
import { useSpApiCredentials } from "./credentials/spApi/useSpApiCredentials";
import ToastCredentialsForm from "./credentials/toast/ToastCredentialsForm";
import { useToastCredentials } from "./credentials/toast/useToastCredentials";

const WRITABLE = new Set<string>(WRITABLE_DESTINATION_DB_TYPES);

interface NewPipelineDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Existing pipeline display-names, for the duplicate-name guard. */
  existingNames: string[];
  /** Called after the file is created so the list can refresh. */
  onCreated: () => void;
}

type Step = 0 | 1 | 2;
const STEP_LABELS = ["Source", "Destination", "Details"] as const;

/** Show the per-step search box once a card grid gets this big. */
const SEARCH_THRESHOLD = 6;

const NAME_RE = /^[a-zA-Z0-9_-]+$/;

const NewPipelineDialog: React.FC<NewPipelineDialogProps> = ({
  open,
  onOpenChange,
  existingNames,
  onCreated
}) => {
  const [step, setStep] = useState<Step>(0);
  const [query, setQuery] = useState("");
  const [sourceId, setSourceId] = useState<string | null>(null);
  const [destinationDb, setDestinationDb] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  // Clear the search when moving between steps so a stale query
  // doesn't hide the next step's cards.
  const goStep = (next: Step) => {
    setQuery("");
    setStep(next);
  };

  const { project } = useCurrentProjectBranch();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const createFile = useCreateFile();
  const saveFile = useSaveFile();
  const navigate = useNavigate();
  const { data: databases, isLoading: databasesLoading } = useDatabases();
  const writableDatabases = (databases ?? []).filter((d) => WRITABLE.has(d.db_type));

  const toastCredentials = useToastCredentials();
  const spApiCredentials = useSpApiCredentials();
  const clickhouseCredentials = useClickHouseCredentials();
  const quickbooksCredentials = useQuickBooksCredentials({
    open,
    projectId: project.id,
    onError: setError,
    // Re-applies the shared dialog fields after a stashed wizard is restored
    // from a full-page OAuth redirect (mobile / popup blocked).
    onRestore: (fields) => {
      setSourceId("quickbooks");
      setDestinationDb(fields.destinationDb);
      setName(fields.name);
      setDescription(fields.description);
      setStep(2);
    }
  });

  const reset = () => {
    setStep(0);
    setQuery("");
    setSourceId(null);
    setDestinationDb(null);
    setName("");
    setDescription("");
    setError(null);
    toastCredentials.reset();
    spApiCredentials.reset();
    quickbooksCredentials.reset();
    clickhouseCredentials.reset();
  };

  const validate = (): boolean => {
    const trimmed = name.trim();
    if (!trimmed) {
      setError("Name is required");
      return false;
    }
    if (!NAME_RE.test(trimmed)) {
      setError("Use only letters, numbers, hyphens and underscores");
      return false;
    }
    if (existingNames.includes(trimmed)) {
      setError("A pipeline with this name already exists");
      return false;
    }
    setError(null);
    return true;
  };

  const handleCreate = async () => {
    if (!sourceId || !destinationDb || !validate()) return;

    const isToast = sourceId === "toast";
    const isQuickbooks = sourceId === "quickbooks";
    const isClickhouse = sourceId === "clickhouse";
    const isSpApi = sourceId === "sp_api";

    let sourceError: string | null = null;
    if (isToast) sourceError = toastCredentials.validate();
    else if (isClickhouse) sourceError = clickhouseCredentials.validate();
    else if (isQuickbooks) sourceError = quickbooksCredentials.validate();
    else if (isSpApi) sourceError = spApiCredentials.validate();
    if (sourceError) {
      setError(sourceError);
      return;
    }

    setCreating(true);
    // A secret that fails to store is reported by `useCreateSecret` ("Failed to
    // create secret"); only a failure after that is the dialog's to report.
    let secretsStored = false;
    try {
      const trimmed = name.trim();

      // Store the client secret / password in the secret manager. A blank
      // value means "reuse an existing secret with this name" — skip create.
      if (isToast) await toastCredentials.persistSecret(trimmed);
      if (isClickhouse) await clickhouseCredentials.persistSecret(trimmed);
      // Both SP-API credentials are pasted rather than captured by an OAuth
      // flow — a refresh token is issued when the app is authorized in Seller
      // Central, so there is nothing for oxy to negotiate.
      if (isSpApi) await spApiCredentials.persistSecret(trimmed);
      // QuickBooks secrets (client secret + rotating refresh token) are
      // stored by the OAuth Connect flow (authorize + callback upserts), so
      // there's nothing to create here.
      secretsStored = true;

      const path = `pipelines/${trimmed}.airway.yml`;
      const pathb64 = encodeBase64(path);
      await createFile.mutateAsync(pathb64);
      await saveFile.mutateAsync({
        pathb64,
        data: buildPipelineScaffold({
          name: trimmed,
          description: description.trim() || undefined,
          sourceId,
          toast: isToast ? toastCredentials.buildScaffoldConfig() : undefined,
          quickbooks: isQuickbooks ? quickbooksCredentials.buildScaffoldConfig() : undefined,
          clickhouse: isClickhouse ? clickhouseCredentials.buildScaffoldConfig() : undefined,
          spApi: isSpApi ? spApiCredentials.buildScaffoldConfig() : undefined,
          destinationDatabase: destinationDb,
          datasetName: trimmed,
          destinationIsAirhouse: writableDatabases
            .find((d) => d.name === destinationDb)
            ?.db_type.startsWith("airhouse")
        })
      });
      onCreated();
      onOpenChange(false);
      reset();
      // Open the YAML editor so the user fills in credentials/endpoints.
      navigate(ROUTES.ORG(orgSlug).WORKSPACE(project.id).IDE.FILES.FILE(pathb64));
    } catch (err) {
      if (secretsStored) {
        toast.error("Failed to create pipeline", {
          description: apiErrorMessage(err, "There was a problem creating the file.")
        });
      }
      console.error("create pipeline failed:", err);
    } finally {
      setCreating(false);
    }
  };

  const q = query.trim().toLowerCase();
  const filteredSources = SOURCE_OPTIONS.filter(
    (o) =>
      !q ||
      o.label.toLowerCase().includes(q) ||
      o.description.toLowerCase().includes(q) ||
      o.id.includes(q)
  );
  const filteredDbs = writableDatabases.filter(
    (d) => !q || d.name.toLowerCase().includes(q) || d.db_type.toLowerCase().includes(q)
  );

  const searchBox = (placeholder: string) => (
    <Input
      autoFocus
      value={query}
      onChange={(e) => setQuery(e.target.value)}
      placeholder={placeholder}
      className='mb-2'
    />
  );

  const clearError = () => setError(null);

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) reset();
        onOpenChange(next);
      }}
    >
      <DialogContent className='flex max-h-[85dvh] flex-col sm:max-w-lg'>
        <DialogHeader>
          <DialogTitle>New pipeline</DialogTitle>
          <DialogDescription>
            {step === 0 && "Where does the data come from?"}
            {step === 1 && "Which database should it land in?"}
            {step === 2 && "Name it — source credentials are filled in the YAML editor next."}
          </DialogDescription>
        </DialogHeader>

        <div className='flex items-center gap-2 py-1'>
          {STEP_LABELS.map((labelText, i) => (
            <div key={labelText} className='flex items-center gap-2'>
              <span
                className={cn(
                  "flex h-5 w-5 items-center justify-center rounded-full text-xs",
                  i === step
                    ? "bg-primary text-primary-foreground"
                    : i < step
                      ? "bg-primary/20 text-primary"
                      : "bg-muted text-muted-foreground"
                )}
              >
                {i + 1}
              </span>
              <span
                className={cn(
                  "text-xs",
                  i === step ? "font-medium text-foreground" : "text-muted-foreground"
                )}
              >
                {labelText}
              </span>
              {i < STEP_LABELS.length - 1 && <span className='text-muted-foreground'>·</span>}
            </div>
          ))}
        </div>

        <div className='-mx-1 min-h-0 flex-1 overflow-y-auto px-1 py-2'>
          {step === 0 && (
            <div>
              {SOURCE_OPTIONS.length > SEARCH_THRESHOLD && searchBox("Search sources…")}
              {filteredSources.length === 0 ? (
                <p className='py-6 text-center text-muted-foreground text-sm'>
                  No source matches “{query}”.
                </p>
              ) : (
                <div className='grid max-h-72 grid-cols-2 gap-2 overflow-y-auto'>
                  {filteredSources.map((opt) => (
                    <ConnectorCard
                      key={opt.id}
                      label={opt.label}
                      description={opt.description}
                      testId={`pipeline-source-${opt.id}`}
                      selected={sourceId === opt.id}
                      onSelect={() => {
                        setSourceId(opt.id);
                        goStep(1);
                      }}
                    />
                  ))}
                </div>
              )}
            </div>
          )}

          {step === 1 &&
            (databasesLoading ? (
              <p className='py-6 text-center text-muted-foreground text-sm'>Loading databases…</p>
            ) : writableDatabases.length === 0 ? (
              <p className='rounded-md border border-border bg-muted/40 px-3 py-4 text-center text-muted-foreground text-sm'>
                No airway-writable database in config.yml. Add a Postgres or Airhouse database in
                Settings, then create a pipeline.
              </p>
            ) : (
              <div>
                {writableDatabases.length > SEARCH_THRESHOLD &&
                  searchBox("Search databases by name or type…")}
                {filteredDbs.length === 0 ? (
                  <p className='py-6 text-center text-muted-foreground text-sm'>
                    No database matches “{query}”.
                  </p>
                ) : (
                  <div className='grid max-h-72 grid-cols-2 gap-2 overflow-y-auto'>
                    {filteredDbs.map((db) => (
                      <ConnectorCard
                        key={db.name}
                        label={db.name}
                        description={db.db_type}
                        testId={`pipeline-dest-${db.name}`}
                        selected={destinationDb === db.name}
                        onSelect={() => {
                          setDestinationDb(db.name);
                          goStep(2);
                        }}
                      />
                    ))}
                  </div>
                )}
              </div>
            ))}

          {step === 2 && (
            <div className='grid gap-4'>
              <div className='rounded-md border border-border bg-muted/40 px-3 py-2 text-muted-foreground text-xs'>
                {SOURCE_OPTIONS.find((s) => s.id === sourceId)?.label}
                <ArrowRight className='mx-2 inline h-3 w-3' />
                {destinationDb}
              </div>
              <div className='grid gap-2'>
                <Label htmlFor='pipeline-name'>Name</Label>
                <Input
                  id='pipeline-name'
                  autoFocus
                  value={name}
                  onChange={(e) => {
                    setName(e.target.value);
                    setError(null);
                  }}
                  placeholder='shopify_raw'
                  className={error ? "border-destructive" : ""}
                />
              </div>
              <div className='grid gap-2'>
                <Label htmlFor='pipeline-description'>Description</Label>
                <Textarea
                  id='pipeline-description'
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                  placeholder='Optional — what this pipeline ingests'
                  rows={2}
                />
              </div>

              {sourceId === "sp_api" && (
                <SpApiCredentialsForm credentials={spApiCredentials} onClearError={clearError} />
              )}
              {sourceId === "toast" && (
                <ToastCredentialsForm credentials={toastCredentials} onClearError={clearError} />
              )}

              {sourceId === "quickbooks" && (
                <QuickBooksCredentialsForm
                  credentials={quickbooksCredentials}
                  onClearError={clearError}
                  onConnect={() =>
                    quickbooksCredentials.handleConnect(destinationDb, name, description)
                  }
                />
              )}

              {sourceId === "clickhouse" && (
                <ClickHouseCredentialsForm
                  credentials={clickhouseCredentials}
                  onClearError={clearError}
                  onError={setError}
                />
              )}

              {error && <p className='text-destructive text-sm'>{error}</p>}
            </div>
          )}
        </div>

        <DialogFooter className='sm:justify-between'>
          {step > 0 ? (
            <Button variant='ghost' onClick={() => goStep((step - 1) as Step)} disabled={creating}>
              <ArrowLeft className='h-4 w-4' />
              Back
            </Button>
          ) : (
            <Button variant='ghost' onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
          )}
          {step === 2 && (
            <Button
              onClick={handleCreate}
              disabled={creating || !name.trim()}
              data-testid='pipeline-create-button'
            >
              {creating ? "Creating..." : "Create"}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default NewPipelineDialog;
