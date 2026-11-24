import type { WorldModel } from "@/types/worldModel";

/**
 * The three counts the world model page states under its heading. A model may
 * list the same entity twice, so entities are counted by id, and a pair of
 * entities connected both ways counts once per direction.
 *
 * Pure, so what the page states is testable without rendering it.
 */
export function worldModelCounts(model: WorldModel) {
  const entities = [...new Map(model.entities.map((e) => [e.id, e])).values()];
  return {
    entities: entities.length,
    connections: new Set(model.edges.map((e) => `${e.from}->${e.to}`)).size,
    measures: entities.reduce((n, e) => n + e.own_measures.length, 0)
  };
}
