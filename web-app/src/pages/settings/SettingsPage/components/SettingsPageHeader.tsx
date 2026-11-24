import { ChevronLeft, X } from "lucide-react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";

export interface SettingsPageHeaderProps {
  /** "Settings", or the section's name once a phone is inside one. */
  title: string;
  /**
   * Set on a phone inside a section: an **absolute** link back to the list, so it can be
   * opened in a tab. Absolute because the leaf route is a splat, and React Router resolves
   * a relative link on one against the full pathname — `"."` was the section itself.
   */
  backHref?: string;
  onClose: () => void;
}

/**
 * The bar across the top of settings: where you are, the way back on a phone, and the way out.
 *
 * Back is a `Link`, not a button calling `history.back()`: it names the list, so it behaves
 * the same whether you arrived by tapping a row or by following a link straight to a section —
 * the second case has no history to go back to.
 */
export function SettingsPageHeader({ title, backHref, onClose }: SettingsPageHeaderProps) {
  return (
    <header className='flex h-14 shrink-0 items-center gap-1 border-border border-b px-3 sm:px-4'>
      {backHref && (
        <Button variant='ghost' size='icon' asChild className='size-9 shrink-0'>
          <Link to={backHref} aria-label='Back to all settings'>
            <ChevronLeft className='size-5' />
          </Link>
        </Button>
      )}
      <h1 className='min-w-0 flex-1 truncate font-medium text-base'>{title}</h1>
      <Button
        variant='ghost'
        size='icon'
        onClick={onClose}
        aria-label='Close settings'
        data-testid='settings-close'
        className='size-9 shrink-0 text-muted-foreground hover:text-foreground'
      >
        <X className='size-5' />
      </Button>
    </header>
  );
}
