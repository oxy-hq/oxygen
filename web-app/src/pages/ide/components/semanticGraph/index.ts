/**
 * The shared kit behind the IDE's semantic-graph surfaces — World Model and
 * Metric Tree. Both draw a laid-out graph of semantic-layer objects with a
 * detail panel beside it, and they are meant to look like one product.
 *
 * What lives here is *appearance and canvas behavior*: the card shell, the edge
 * router, the canvas chrome, the ELK runner, the panel primitives, the shared
 * geometry. What a node *means* — an entity's measure chips, a measure's role —
 * stays with the surface that owns the domain.
 */

export { NODE_HEIGHT_COLLAPSED, NODE_WIDTH, PANEL_WIDTH } from "./constants";
export {
  layoutWithElk,
  type NodeSize,
  type WaypointMap
} from "./elkLayout";
export { GRAPH_EDGE_TYPE, GraphCanvas } from "./GraphCanvas";

export { GraphNodeCard, GraphNodeHandles } from "./GraphNodeCard";
export {
  CONFIDENCE_HELP,
  FORM_HELP,
  formatMeasureValue,
  InfoTip,
  MagnitudeBar,
  MetaBadge,
  Row,
  SectionHeader,
  SectionSpinner
} from "./panelPrimitives";
