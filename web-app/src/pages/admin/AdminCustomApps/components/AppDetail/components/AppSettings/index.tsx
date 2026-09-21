import { CheckCircle2, ExternalLink, EyeOff, Send, Trash2, Triangle } from "lucide-react";
import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { usePublishApp, useUnpublishApp } from "@/hooks/api/customApps/useCustomApps";
import { useDeleteApp } from "@/hooks/api/customApps/useDeleteApp";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { CustomApp } from "@/types/apps";

function formatTimestamp(value: string): string {
  return new Date(value).toLocaleString();
}

/**
 * Read-then-act surface for one app. Three rows: re-sync (S3 only),
 * delete, and a pointer to the bootstrap PR if one exists.
 *
 * v1 is intentionally minimal — manifest_override editing, branch
 * changes, role grants etc. land in follow-ups. Today this just
 * surfaces what the existing API already supports.
 */
export const AppSettings = ({ app }: { app: CustomApp }) => {
  const navigate = useNavigate();
  const { mutate: del, isPending: isDeleting } = useDeleteApp();
  const { mutate: publish, isPending: isPublishing } = usePublishApp();
  const { mutate: unpublish, isPending: isUnpublishing } = useUnpublishApp();
  const isPublished = !!app.published_at;

  const handleDelete = () => {
    if (
      !window.confirm(
        `Delete "${app.name}"? The bundle in oxy-hq/customer-apps stays — only the registration row goes.`
      )
    ) {
      return;
    }
    del(app.id, {
      onSuccess: () => navigate("/admin/apps")
    });
  };

  return (
    <div className='space-y-3 p-4 pt-0'>
      <SettingRow
        title={isPublished ? "Published" : "Draft"}
        description={
          isPublished
            ? `Live for members of org ${app.org_slug}.`
            : "Only Oxy staff can reach this app. Publish to show it in the customer sidebar."
        }
        meta={app.published_at ? `Last published ${formatTimestamp(app.published_at)}` : undefined}
        tone={isPublished ? "success" : undefined}
        action={
          isPublished ? (
            <>
              <Button
                variant='outline'
                size='sm'
                disabled={isPublishing}
                onClick={() => publish(app.id)}
              >
                <Send className='size-3.5' />
                {isPublishing ? "Re-publishing…" : "Re-publish"}
              </Button>
              <Button
                variant='ghost'
                size='sm'
                disabled={isUnpublishing}
                onClick={() => unpublish(app.id)}
              >
                <EyeOff className='size-3.5' />
                {isUnpublishing ? "Unpublishing…" : "Unpublish"}
              </Button>
            </>
          ) : (
            <Button size='sm' disabled={isPublishing} onClick={() => publish(app.id)}>
              <CheckCircle2 className='size-3.5' />
              {isPublishing ? "Publishing…" : "Publish"}
            </Button>
          )
        }
      />

      {app.bootstrap_pr_url && (
        <SettingRow
          title='Bootstrap PR'
          description='Merge to seed the customer-apps repo.'
          action={
            <Button variant='outline' size='sm' asChild>
              <a href={app.bootstrap_pr_url} target='_blank' rel='noopener noreferrer'>
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
          <Button variant='destructive' size='sm' disabled={isDeleting} onClick={handleDelete}>
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
  tone,
  disabledNote,
  meta
}: {
  title: string;
  description: string;
  action: React.ReactNode;
  tone?: "destructive" | "success";
  disabledNote?: string;
  meta?: string;
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
    <div className={`rounded-lg border p-4 ${toneClass}`}>
      <div className='min-w-0'>
        <div className={`font-medium text-xs ${titleToneClass}`}>{title}</div>
        <p className='mt-1 text-muted-foreground text-xs leading-relaxed'>{description}</p>
        {meta && (
          <p className='mt-2 font-mono text-muted-foreground text-xs tabular-nums'>{meta}</p>
        )}
        {disabledNote && (
          <p className='mt-2 font-mono text-muted-foreground text-xs'>{disabledNote}</p>
        )}
      </div>
      <div className='mt-3 flex flex-wrap items-center gap-2'>{action}</div>
    </div>
  );
};
