import { useState } from "react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogFooter,
  AlertDialogTitle
} from "@/components/ui/shadcn/alert-dialog";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import type { WidgetButton as WidgetButtonData } from "@/types/homeWidgets";

/**
 * One of a widget's buttons. When the server says it must ask first (the app's
 * manifest or its view asked), the press waits for a yes in a dialog that says
 * exactly what will happen.
 */
export function WidgetButton({
  button,
  testId,
  pending,
  disabled,
  secondary,
  onPress
}: {
  button: WidgetButtonData;
  testId: string;
  pending: boolean;
  disabled: boolean;
  secondary?: boolean;
  onPress: () => void;
}) {
  const [asking, setAsking] = useState(false);
  const variant = button.destructive ? "destructive" : secondary ? "outline" : "default";
  return (
    <>
      <Button
        size='sm'
        variant={variant}
        data-testid={testId}
        disabled={disabled}
        onClick={() => (button.confirm ? setAsking(true) : onPress())}
      >
        {pending && <Spinner className='size-3.5' />}
        {button.label}
      </Button>
      {button.confirm && (
        <AlertDialog open={asking} onOpenChange={setAsking}>
          <AlertDialogContent className='sm:max-w-sm' data-testid={`${testId}-confirm`}>
            <AlertDialogTitle className='font-medium text-base'>{button.confirm}</AlertDialogTitle>
            <AlertDialogFooter>
              <AlertDialogCancel data-testid={`${testId}-cancel`}>Cancel</AlertDialogCancel>
              <AlertDialogAction
                data-testid={`${testId}-yes`}
                className={
                  button.destructive
                    ? "bg-destructive text-destructive-foreground hover:bg-destructive/90"
                    : undefined
                }
                onClick={onPress}
              >
                {button.label}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      )}
    </>
  );
}
