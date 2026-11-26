import { ExternalLink, Trash2, Triangle } from "lucide-react";
import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { useDeleteApp } from "@/hooks/api/customApps/useDeleteApp";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { CustomApp } from "@/types/apps";
import { AppLifecycle } from "../../../AppLifecycle";

/**
 * Read-then-act surface for one app: the lifecycle card, a pointer to the bootstrap
 * PR if one exists, and delete.
 *
 * The Published/Draft row this used to open with — Publish / Re-publish / Unpublish
 * against `POST|DELETE /customer-apps/{id}/publish` — is the lifecycle card now, the
 * same one the console and the build history show, so promoting and unpublishing
 * read and confirm the same way here as everywhere else.
 *
 * `lifecycle={false}` is for a host that already shows the card above this panel —
 * the dossier does, in its "Live build & history" section — so one app never has two
 * cards, two confirms and two copies of every `app-lifecycle-*` testid on one screen.
 */
export const AppSettings = ({ app, lifecycle = true }: { app: CustomApp; lifecycle?: boolean }) => {
  const navigate = useNavigate();
  const { mutate: del, isPending: isDeleting } = useDeleteApp();

  const handleDelete = () => {
    if (
      !window.confirm(
        `Delete "${app.name}"? The bundle in oxy-hq/customer-apps stays — only the registration row goes.`
      )
    ) {
      return;
    }
    del(app.id, {
      onSuccess: () => {
        void navigate("/admin/apps");
      }
    });
  };

  return (
    <div className='space-y-3 p-4 pt-0' data-testid='admin-app-settings'>
      {lifecycle && <AppLifecycle app={app} />}

      {app.bootstrap_pr_url && (
        <SettingRow
          title='Bootstrap PR'
          description='Merge to seed the customer-apps repo.'
          action={
            <Button variant='outline' size='sm' asChild>
              <a
                href={app.bootstrap_pr_url}
                target='_blank'
                rel='noopener noreferrer'
                data-testid='admin-app-settings-bootstrap-pr'
              >
                <Triangle className='size-3.5' />
                Open PR
                <ExternalLink className='size-3.5' />
              </a>
            </Button>
          }
        />
      )}

      <SettingRow
        title='Delete registration'
        description='Removes the app row. The bundle source stays — clean that up separately.'
        tone='destructive'
        action={
          <Button
            variant='destructive'
            size='sm'
            disabled={isDeleting}
            onClick={handleDelete}
            data-testid='admin-app-settings-delete'
          >
            <Trash2 className='size-3.5' />
            {isDeleting ? "Deleting…" : "Delete"}
          </Button>
        }
      />
    </div>
  );
};

const SettingRow = ({
  title,
  description,
  action,
  tone
}: {
  title: string;
  description: string;
  action: React.ReactNode;
  tone?: "destructive" | "success";
}) => {
  const toneClass =
    tone === "destructive"
      ? "border-destructive/30 bg-destructive/5"
      : tone === "success"
        ? "border-success/30 bg-success/5"
        : "bg-card";
  const titleToneClass =
    tone === "destructive" ? "text-destructive" : tone === "success" ? ADMIN_TONE.ok.text : "";
  return (
    // Stacked, not side-by-side: the dossier is a narrow resizable column (and
    // an overlay Sheet on narrow viewports), so a title|action row can't hold a
    // two-button action without overflowing. The action sits below the copy and
    // wraps.
    <div className={` border p-4 ${toneClass}`}>
      <div className='min-w-0'>
        <div className={`font-medium text-xs ${titleToneClass}`}>{title}</div>
        <p className='mt-1 text-muted-foreground text-xs leading-relaxed'>{description}</p>
      </div>
      <div className='mt-3 flex flex-wrap items-center gap-2'>{action}</div>
    </div>
  );
};
