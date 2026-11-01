import { isCalm, isUnavailable } from "@/pages/launcher/CustomerHome/components/HomeWidgets/utils";
import type { WidgetCard } from "@/types/homeWidgets";

export type PanelGroups = {
  /** Something to do: a warning, a critical level, or buttons. */
  needs: WidgetCard[];
  /** Nothing to do. */
  calm: WidgetCard[];
  /** The app's function did not answer. */
  failed: WidgetCard[];
};

/**
 * The panel's order, the home's: what needs the person first (a critical one
 * before a warning, otherwise the layout's order), then the calm copies, then
 * the ones that could not load, which are their app owner's news.
 */
export function groupForPanel(widgets: WidgetCard[]): PanelGroups {
  const critical = (card: WidgetCard) => (card.view?.level === "critical" ? 0 : 1);
  return {
    needs: widgets
      .filter((card) => !isUnavailable(card) && !isCalm(card))
      // Array.prototype.sort is stable: equal levels keep the layout's order.
      .sort((a, b) => critical(a) - critical(b)),
    calm: widgets.filter(isCalm),
    failed: widgets.filter(isUnavailable)
  };
}

/** A row's test id: the card's app, widget and copy. */
export function rowTestId(card: WidgetCard): string {
  return `topbar-today-row-${card.app.slug}-${card.id}${card.instance ? `-${card.instance}` : ""}`;
}
