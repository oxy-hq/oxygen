import { createContext, useContext } from "react";

/**
 * The workspace the org's pages load from (`OrgWorkspace`): the address names
 * only the org, `/<org>/<page>`. `undefined` in local mode, which has one.
 */
const ChosenWorkspaceContext = createContext<string | undefined>(undefined);

export const ChosenWorkspaceProvider = ChosenWorkspaceContext.Provider;

export const useChosenWorkspaceId = (): string | undefined => useContext(ChosenWorkspaceContext);
