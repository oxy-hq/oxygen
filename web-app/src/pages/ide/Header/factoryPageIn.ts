import type ROUTES from "@/libs/utils/routes";

type IdeRoutes = ReturnType<ReturnType<(typeof ROUTES)["ORG"]>["WORKSPACE"]>["IDE"];

/**
 * The page to open in another workspace's Factory, so moving between workspaces
 * keeps the person where they were: the same section, and the same tab of the
 * coordinator and observability. Never an id inside one (a file, a trace, a run, an
 * experiment): those belong to the workspace being left. Anything else, the
 * Factory's landing.
 *
 * `currentRoot` is the Factory root of the workspace being left, and `pathname`
 * the page in it.
 */
export function factoryPageIn(target: IdeRoutes, currentRoot: string, pathname: string): string {
  const rest = pathname.startsWith(`${currentRoot}/`) ? pathname.slice(currentRoot.length + 1) : "";
  const [section, tab] = rest.split("/");
  switch (section) {
    case "files":
      return target.FILES.ROOT;
    case "database":
      return target.DATABASE.ROOT;
    case "semantic":
      return target.SEMANTIC.ROOT;
    case "world-model":
      return target.WORLD_MODEL.ROOT;
    case "experiments":
      return target.EXPERIMENTS.ROOT;
    case "edge":
      return target.EDGE.ROOT;
    case "coordinator":
      return tab ? `${target.COORDINATOR.ROOT}/${tab}` : target.COORDINATOR.OVERVIEW;
    case "observability":
      return tab ? `${target.OBSERVABILITY.ROOT}/${tab}` : target.OBSERVABILITY.TRACES;
    default:
      return target.ROOT;
  }
}
