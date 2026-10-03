import { ArrowUp, CircleX } from "lucide-react";
import type { ReactNode, RefObject } from "react";
import { HighlightTextarea } from "@/components/ui/HighlightTextarea";
import { Button } from "@/components/ui/shadcn/button";

interface MessageInputShellProps {
  value: string;
  onChange: (e: React.ChangeEvent<HTMLTextAreaElement>) => void;
  onKeyDown: (e: React.KeyboardEvent<HTMLTextAreaElement>) => void;
  onSend: () => void;
  onStop: () => void;
  disabled: boolean;
  isLoading?: boolean;
  showWarning?: boolean;
  placeholder?: string;
  sendDisabled?: boolean;
  textareaRef?: RefObject<HTMLTextAreaElement | null>;
  onSelect?: (e: React.SyntheticEvent<HTMLTextAreaElement>) => void;
  onClick?: (e: React.SyntheticEvent<HTMLTextAreaElement>) => void;
  aboveInput?: ReactNode;
  extraActions?: ReactNode;
  highlight?: ReactNode;
}

const MessageInputShell = ({
  value,
  onChange,
  onKeyDown,
  onSend,
  onStop,
  disabled,
  isLoading = false,
  showWarning = false,
  placeholder = "Ask a follow-up question...",
  sendDisabled,
  textareaRef,
  onSelect,
  onClick,
  aboveInput,
  extraActions,
  highlight
}: MessageInputShellProps) => {
  const isSendDisabled = sendDisabled ?? (!value.trim() || disabled);

  return (
    <div className='flex w-full flex-1 flex-col gap-1.5'>
      {showWarning && (
        <div className='w-full rounded-lg border border-border bg-muted p-2'>
          <div className='flex items-center'>
            <svg
              aria-hidden='true'
              className='mr-2 h-5 w-5 text-warning'
              fill='currentColor'
              viewBox='0 0 20 20'
            >
              <path
                fillRule='evenodd'
                d='M8.257 3.099c.765-1.36 2.722-1.36 3.486 0l5.58 9.92c.75 1.334-.213 2.98-1.742 2.98H4.42c-1.53 0-2.493-1.646-1.743-2.98l5.58-9.92zM11 13a1 1 0 11-2 0 1 1 0 012 0zm-1-8a1 1 0 00-1 1v3a1 1 0 002 0V6a1 1 0 00-1-1z'
                clipRule='evenodd'
              />
            </svg>
            <span className='font-medium text-foreground text-sm'>
              You've asked a lot of questions. You may want to start a new thread for optimal
              performance.
            </span>
          </div>
        </div>
      )}
      <div className='relative'>
        {aboveInput}
        <div className='overflow-hidden rounded-md border border-border bg-background shadow-md transition-shadow focus-within:border-ring focus-within:ring-[3px] focus-within:ring-ring/50'>
          <HighlightTextarea
            ref={textareaRef}
            value={value}
            onChange={onChange}
            onSelect={onSelect}
            onClick={onClick}
            onKeyDown={onKeyDown}
            placeholder={placeholder}
            className='h-14 max-h-20 resize-none overflow-y-auto rounded-none border-0 bg-transparent text-sm shadow-none placeholder:text-sm focus-visible:ring-0'
            overlayClassName='px-3 py-2 text-sm'
            disabled={disabled}
            highlight={highlight}
          />
          <div className='flex items-center justify-end gap-2 border-border border-t bg-background px-2 py-1.5'>
            {extraActions}
            {isLoading ? (
              <Button
                size='icon'
                className='size-7'
                onClick={onStop}
                data-testid='message-input-stop-button'
                aria-label='Stop run'
              >
                <CircleX className='size-4' />
              </Button>
            ) : (
              <Button
                size='icon'
                className='size-7'
                onClick={onSend}
                disabled={isSendDisabled}
                data-testid='message-input-send-button'
                aria-label='Send message'
              >
                <ArrowUp className='size-4' />
              </Button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};

export default MessageInputShell;
