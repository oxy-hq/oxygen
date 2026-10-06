import type React from "react";
import { useId } from "react";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { Label } from "@/components/ui/shadcn/label";
import type { TokenOptions } from "@/types/apiToken";
import { type AccessAction, type AccessDraft, orgsRefusingAllAccess } from "../../accessDraft";
import OptionChip from "../OptionChip";
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
 */
const AccessFields: React.FC<Props> = ({
  draft,
  dispatch,
  options,
  optionsLoading,
  optionsFailed,
  onRetryOptions
}) => {
  const group = useId();
  const refusing = orgsRefusingAllAccess(options);

  return (
    <fieldset className='flex min-w-0 flex-col gap-2 border-0 p-0'>
      <Label asChild>
        <legend>Access</legend>
      </Label>
      <div className='flex flex-wrap items-center gap-1.5'>
        <OptionChip
          group={group}
          testId='account-token-access-all'
          checked={draft.mode === "all"}
          onSelect={() => dispatch({ type: "set_mode", mode: "all" })}
          label='All access'
        />
        <OptionChip
          group={group}
          testId='account-token-access-selected'
          checked={draft.mode === "selected"}
          onSelect={() => dispatch({ type: "set_mode", mode: "selected" })}
          label='Selected workspaces'
        />
      </div>

      {draft.mode === "all" ? (
        <>
          <p className='text-muted-foreground text-xs'>
            Everything you can reach, in every organization and workspace, including ones you join
            later.
          </p>
          {refusing.length > 0 && (
            <Caution testId='account-token-all-access-caution'>
              {listNames(refusing)} {refusing.length === 1 ? "doesn't" : "don't"} accept all-access
              tokens. Select workspaces instead to use the token there.
            </Caution>
          )}
        </>
      ) : (
        <GrantPicker
          draft={draft}
          dispatch={dispatch}
          options={options}
          loading={optionsLoading}
          failed={optionsFailed}
          onRetry={onRetryOptions}
        />
      )}

      {(options?.can_platform || options?.can_partner) && (
        <div className='mt-1 flex flex-col gap-2 border-t pt-3'>
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
      )}
    </fieldset>
  );
};

export default AccessFields;
