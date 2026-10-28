import { createContext, useContext } from "react";

/**
 * The workspace a page loads from when the URL does not name one: the org home
 * (`/<org>/home`) shows every app in the org but still talks to one workspace
 * for chat, threads, setup readiness and settings. `undefined` on a workspace
 * route, where the `:wsId` URL segment names it.
 */
const ChosenWorkspaceContext = createContext<string | undefined>(undefined);

export const ChosenWorkspaceProvider = ChosenWorkspaceContext.Provider;

export const useChosenWorkspaceId = (): string | undefined => useContext(ChosenWorkspaceContext);
