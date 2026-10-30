// The org's home layout, and the description of the home that
// `GET /api/orgs/{org_id}/home` answers: types only, for an agent or a script
// that reads the home, edits `effective_layout` and writes it back
// (`oxyc home get | … | oxyc home apply`).
//
// The layout is one document: an ordered list of sections, and in a widgets
// section (which may repeat, each a titled group) an ordered list of widgets,
// each `<app>/<widget>` and given a size the widget allows. The same widget may
// appear more than once, each copy with its own `id`, `title` and `params`.
// Placement is by order and named size, never coordinates.
//
// THE SERVER IS THE AUTHORITY. It validates a layout on write and answers every
// problem at once (`layout_invalid`); these types only describe the shapes.
//
// Nothing past ES2020, and no imports beyond the widget size type.

import type { OxyAppWidgetSize, WidgetParamsSchema, WidgetParamValue } from "./widgets";

/** A widget's size in a layout: the same names a manifest's `size` and `sizes` use. */
export type WidgetSize = OxyAppWidgetSize;

/** One widget placed in a widgets section. */
export interface HomeLayoutItem {
  /** `<app slug>/<widget id>`, `"bookkeeping/close"`. */
  widget: string;
  /**
   * Names this copy when a widget appears more than once (`"mission"`): the
   * copy is `<app>/<widget>#<id>`. Same segment rule as a widget id.
   */
  id?: string;
  /** Shown instead of the widget's own title, ≤ 40 characters. */
  title?: string;
  /** This copy's settings, checked against the widget's `params` schema. */
  params?: Record<string, WidgetParamValue>;
  /** One of the widget's `sizes`; absent, its default `size`. */
  size?: WidgetSize;
  /** Listed but not drawn. */
  hidden?: boolean;
}

/** A widgets section, a group on the home: listed widgets first, in order. */
export interface HomeLayoutWidgetsSection {
  kind: "widgets";
  /** The group's heading, ≤ 40 characters; absent, the home says "Widgets". */
  title?: string;
  items?: HomeLayoutItem[];
  /**
   * What happens to widgets not listed: `show` (default) draws them after the
   * listed ones at their default size; `hide` leaves them out. Allowed on the
   * first widgets section only.
   */
  others?: "show" | "hide";
}

/** "Your apps". */
export interface HomeLayoutAppsSection {
  kind: "apps";
}

/** The world model and recent questions. */
export interface HomeLayoutInsightsSection {
  kind: "insights";
}

/**
 * A section of the home. `apps` and `insights` at most once, `widgets` as many
 * times as there are groups; a kind left out is not drawn.
 */
export type HomeLayoutSection =
  | HomeLayoutWidgetsSection
  | HomeLayoutAppsSection
  | HomeLayoutInsightsSection;

/** The layout document. */
export interface HomeLayout {
  version: 1;
  /** The order the home draws its sections in. */
  sections: HomeLayoutSection[];
}

/** The app a widget belongs to, as the description names it. */
export interface HomeWidgetApp {
  id: string;
  slug: string;
  name: string;
}

/** A widget the home shows, in the description's `sections`. */
export interface HomeDescribedWidget {
  /** `<app slug>/<widget id>`. */
  widget: string;
  /** This copy: `<app>/<widget>`, or `<app>/<widget>#<id>` when it has an `id`. */
  key: string;
  /** The copy's `id`, or null for the one unnamed copy. */
  id: string | null;
  app: HomeWidgetApp;
  /** The layout item's title, else the widget's. */
  title: string;
  /** This copy's settings, defaults filled in; `{}` when the widget has none. */
  params: Record<string, WidgetParamValue>;
  /** The size it is drawn at. */
  size: WidgetSize;
  /** The sizes a layout may choose. */
  sizes: WidgetSize[];
}

/** What the home draws, in order. */
export type HomeDescribedSection =
  | { kind: "widgets"; title: string | null; items: HomeDescribedWidget[] }
  | { kind: "apps" }
  | { kind: "insights" };

/** Every widget the org's apps declare, shown or not. */
export interface HomeCatalogEntry {
  widget: string;
  title: string;
  /** Its default size. */
  size: WidgetSize;
  sizes: WidgetSize[];
  /** The settings a layout may give it, or null when it declares none. */
  params: WidgetParamsSchema | null;
  /**
   * Some setting is required and has no default, so the widget is never added
   * on its own: a layout entry must say which store, which vendor.
   */
  needs_params: boolean;
  /** Whether the effective layout draws it. */
  shown: boolean;
}

/** The `GET /api/orgs/{org_id}/home` response. */
export interface HomeDescription {
  org: { id: string; slug: string; name: string };
  /** 0 until customized; send it back on a write so two writers never overwrite each other unseen. */
  revision: number;
  customized: boolean;
  /** The stored document, or null for the default. */
  layout: HomeLayout | null;
  /** A complete document for what shows now: edit it and PUT it back. */
  effective_layout: HomeLayout;
  sections: HomeDescribedSection[];
  catalog: HomeCatalogEntry[];
}

/** One thing wrong with a layout, as a `422 layout_invalid` lists it. */
export interface HomeLayoutProblem {
  /** `sections[0].items[2].widget`. */
  path: string;
  message: string;
  /** What to do about it. */
  fix: string;
}

/** The body of `PUT /api/orgs/{org_id}/home/layout`. */
export interface HomeLayoutWrite {
  layout: HomeLayout;
  /** The `revision` last read; a stale one is a 409. Omit to overwrite. */
  revision?: number;
  /** Answer the description the layout would give, saving nothing. */
  dry_run?: boolean;
}
