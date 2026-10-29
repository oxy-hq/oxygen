import { useMutation, useQueryClient } from "@tanstack/react-query";
import { HomeWidgetsService } from "@/services/api/homeWidgets";
import type { HomeWidgetsResponse, WidgetPress } from "@/types/homeWidgets";
import queryKeys from "../queryKey";

/**
 * Press a widget's button. The server runs the action once per press (its
 * idempotency key), so a retry of the same button replays the first answer.
 *
 * When the action answers a view, the card shows it at once; otherwise the
 * org's widgets are read again so the card reflects what the action did.
 */
export function usePressWidget(orgSlug: string) {
  const qc = useQueryClient();
  const key = queryKeys.homeWidgets.list(orgSlug);
  return useMutation({
    mutationFn: (press: WidgetPress) => HomeWidgetsService.press(press),
    onSuccess: (answer, press) => {
      if (!answer.view) {
        qc.invalidateQueries({ queryKey: key });
        return;
      }
      const view = answer.view;
      qc.setQueryData<HomeWidgetsResponse>(key, (old) =>
        old
          ? {
              widgets: old.widgets.map((w) =>
                w.app.id === press.appId && w.id === press.widget ? { ...w, view, error: null } : w
              )
            }
          : old
      );
    }
  });
}
