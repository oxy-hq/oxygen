import type React from "react";
import { useId } from "react";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import type { TokenOptions } from "@/types/apiToken";
import { type AccessAction, type AccessDraft, orgsRefusingAllAccess } from "../../accessDraft";
import OptionChip from "../OptionChip";
import SheetRow from "../SheetRow";
import Caution, { listNames } from "./components/Caution";
import GrantPicker from "./components/GrantPicker";

interface Props {
  draft: AccessDraft;
  dispatch: React.Dispatch<AccessAction>;
  /** `GET /user/token-options`; `undefined` while loading or after a failure. */
  options?: TokenOptions;
  optionsLoading: boolean;
  optionsFailed: boolean;
  onRetryOptions: () => void;
  /**
   * Create only: a third type beside the two access modes, for someone who may mint a sandbox
   * agent token. Absent, the type isn't offered — Edit access never passes it, since a sandbox
   * agent token is not something an existing token can become.
   */
  sandbox?: SandboxOption;
}

export interface SandboxOption {
  active: boolean;
  onActiveChange: (active: boolean) => void;
  /** Shown in place of the access picker and the standing options while the type is chosen. */
  fields: React.ReactNode;
}

interface StandingProps extends Pick<Props, "draft" | "dispatch"> {
  flag: "platform" | "partner";
  title: string;
  hint: string;
}

const StandingOption: React.FC<StandingProps> = ({ draft, dispatch, flag, title, hint }) => {
  const id = useId();
  return (
    <div className='flex items-start gap-2'>
      <Checkbox
        id={id}
        className='mt-0.5'
        checked={draft[flag]}
        onCheckedChange={(on) => dispatch({ type: "set_standing", flag, on: on === true })}
        data-testid={`account-token-${flag}`}
      />
      <label htmlFor={id} className='flex cursor-pointer flex-col text-xs'>
        <span className='font-medium'>{title}</span>
        <span className='text-muted-foreground'>{hint}</span>
      </label>
    </div>
  );
};

/**
 * What a token can reach: all access (the default) or selected workspaces, plus the staff and
 * partner options for the people who hold that standing. Shared by create and Edit access.
 *
 * Rows of the token sheet, for a parent that stacks them: the type as a segmented control, then
 * whatever the chosen type asks for on the same value edge.
 */
const AccessFields: React.FC<Props> = ({
  draft,
  dispatch,
  options,
  optionsLoading,
  optionsFailed,
  onRetryOptions,
  sandbox
}) => {
  const group = useId();
  const workspaces = useId();
  const refusing = orgsRefusingAllAccess(options);
  const sandboxActive = !!sandbox?.active;
  // Picking an access mode leaves the sandbox agent type, which shares the one radio group.
  const setMode = (mode: AccessDraft["mode"]) => {
    sandbox?.onActiveChange(false);
    dispatch({ type: "set_mode", mode });
  };

  return (
    <>
      <SheetRow label='Access' labelId={group}>
        <div
          role='radiogroup'
          aria-labelledby={group}
          className='grid h-9 auto-cols-fr grid-flow-col rounded-lg bg-accent p-0.5'
        >
          <OptionChip
            group={group}
            testId='account-token-access-all'
            checked={!sandboxActive && draft.mode === "all"}
            onSelect={() => setMode("all")}
            label='All access'
            variant='segment'
          />
          <OptionChip
            group={group}
            testId='account-token-access-selected'
            checked={!sandboxActive && draft.mode === "selected"}
            onSelect={() => setMode("selected")}
            label='Selected workspaces'
            variant='segment'
          />
          {sandbox && (
            <OptionChip
              group={group}
              testId='account-token-access-sandbox'
              checked={sandboxActive}
              onSelect={() => sandbox.onActiveChange(true)}
              label='Sandbox agent'
              variant='segment'
            />
          )}
        </div>
        {!sandboxActive && draft.mode === "all" && (
          <div className='mt-2 flex flex-col gap-2'>
            <p className='text-muted-foreground text-xs'>
              Everything you can reach, in every organization and workspace, including ones you join
              later.
            </p>
            {refusing.length > 0 && (
              <Caution testId='account-token-all-access-caution'>
                {listNames(refusing)} {refusing.length === 1 ? "doesn't" : "don't"} accept
                all-access tokens. Select workspaces instead to use the token there.
              </Caution>
            )}
          </div>
        )}
      </SheetRow>

      {sandboxActive
        ? sandbox.fields
        : draft.mode === "selected" && (
            <SheetRow label='Workspaces' labelId={workspaces}>
              {/* biome-ignore lint/a11y/useSemanticElements: a fieldset can't sit on the sheet's value edge beside its own label. */}
              <div role='group' aria-labelledby={workspaces}>
                <GrantPicker
                  draft={draft}
                  dispatch={dispatch}
                  options={options}
                  loading={optionsLoading}
                  failed={optionsFailed}
                  onRetry={onRetryOptions}
                />
              </div>
            </SheetRow>
          )}

      {/* Not for a sandbox agent token: it carries neither standing, whatever the caller holds. */}
      {!sandboxActive && (options?.can_platform || options?.can_partner) && (
        <SheetRow>
          <div className='flex flex-col gap-2'>
            {options.can_platform && (
              <StandingOption
                draft={draft}
                dispatch={dispatch}
                flag='platform'
                title='Include staff access'
                hint='The token can act with your Oxygen staff standing, such as in the admin console.'
              />
            )}
            {options.can_partner && (
              <StandingOption
                draft={draft}
                dispatch={dispatch}
                flag='partner'
                title='Include partner access'
                hint='The token can reach your client organizations as a partner.'
              />
            )}
          </div>
        </SheetRow>
      )}
    </>
  );
};

export default AccessFields;
