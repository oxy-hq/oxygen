import { Check, Pencil, X } from "lucide-react";
import type React from "react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { tokenErrorMessage } from "@/hooks/api/userTokens/tokenErrors";
import { useRenameUserToken } from "@/hooks/api/userTokens/useUserTokenMutations";
import type { Token } from "@/types/apiToken";

/** The server's limit: a name is 1 to 100 characters once trimmed. */
export const TOKEN_NAME_MAX = 100;

interface Props {
  token: Pick<Token, "id" | "name">;
  /** Off for a revoked token: nothing about it changes any more. */
  editable: boolean;
}

interface EditorProps {
  token: Pick<Token, "id" | "name">;
  onDone: () => void;
}

/**
 * Mounted on entry and unmounted on Save or Cancel, so the box always starts from the name as it
 * is now. A refusal is shown beside the box, since it is about the text still in it.
 */
const NameEditor: React.FC<EditorProps> = ({ token, onDone }) => {
  const [name, setName] = useState(token.name);
  const [error, setError] = useState<string | null>(null);
  const rename = useRenameUserToken();
  const trimmed = name.trim();

  const save = async () => {
    if (!trimmed || rename.isPending) return;
    // Nothing changed: leave without a request.
    if (trimmed === token.name) return onDone();
    setError(null);
    try {
      await rename.mutateAsync({ id: token.id, name: trimmed });
      onDone();
    } catch (err) {
      setError(tokenErrorMessage(err, "rename"));
    }
  };

  return (
    // Wider than the name's column: it lies over the cells beside it while it is open.
    <div className='relative z-10 ml-1 flex w-max flex-col gap-1 bg-background pr-2'>
      <div className='flex items-center gap-1'>
        <Input
          value={name}
          onChange={(event) => setName(event.target.value)}
          onFocus={(event) => event.currentTarget.select()}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              void save();
            }
            if (event.key === "Escape") onDone();
          }}
          maxLength={TOKEN_NAME_MAX}
          autoComplete='off'
          autoFocus
          className='h-7 w-44 text-xs'
          aria-label={`New name for ${token.name}`}
          aria-invalid={!!error}
          data-testid='account-token-rename-input'
        />
        <Button
          variant='ghost'
          size='icon'
          className='size-7'
          onClick={save}
          disabled={!trimmed || rename.isPending}
          title='Save name'
          aria-label={`Save the new name for ${token.name}`}
          data-testid='account-token-rename-save'
        >
          {rename.isPending ? <Spinner className='size-3' /> : <Check className='size-3.5' />}
        </Button>
        <Button
          variant='ghost'
          size='icon'
          className='size-7 text-muted-foreground'
          onClick={onDone}
          disabled={rename.isPending}
          title='Cancel'
          aria-label={`Keep the name ${token.name}`}
          data-testid='account-token-rename-cancel'
        >
          <X className='size-3.5' />
        </Button>
      </div>
      {error && (
        <p
          className='font-normal text-destructive text-xs'
          role='alert'
          data-testid='account-token-rename-error'
        >
          {error}
        </p>
      )}
    </div>
  );
};

/**
 * A token's name, renamed in place. A name is only a label: changing it touches neither the
 * secret nor the access.
 */
const TokenName: React.FC<Props> = ({ token, editable }) => {
  const [editing, setEditing] = useState(false);

  if (editing) return <NameEditor token={token} onDone={() => setEditing(false)} />;

  return (
    <span className='inline-flex max-w-full items-center gap-1'>
      <span className='truncate' data-testid='account-token-name-text'>
        {token.name}
      </span>
      {editable && (
        <Button
          variant='ghost'
          size='icon'
          // Quiet until the row is pointed at or tabbed to; always shown where there is no hover.
          // It takes no width until then, so a name has the whole of its narrow column.
          className='h-6 w-0 min-w-0 shrink-0 overflow-hidden p-0 text-muted-foreground opacity-0 focus-visible:w-6 focus-visible:opacity-100 group-hover/row:w-6 group-hover/row:opacity-100 [@media(hover:none)]:w-6 [@media(hover:none)]:opacity-100'
          onClick={() => setEditing(true)}
          title='Rename'
          aria-label={`Rename ${token.name}`}
          data-testid='account-token-rename-button'
        >
          <Pencil className='size-3' />
        </Button>
      )}
    </span>
  );
};

export default TokenName;
