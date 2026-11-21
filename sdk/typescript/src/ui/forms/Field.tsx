// Started from pokehouse-oxy `apps/warehouse/src/components/Field.tsx` (INVENTORY section 2: the
// tested copy, `field.test.ts`), itself the reference's (oxy-hq/reference-app@3caad1f
// `src/components/Field.tsx`). Kept: the label above the control in the `label` role, then the
// control, then the error OR the hint (never both) under one id, the error a `role="alert"`;
// `labelHint` beside the label but outside the <label> (a hint trigger is focusable); a hint
// that can be any node; `describedBy(id)`. `htmlFor` is warehouse's name for the control's id
// and `id` the reference's: both work, so either app's move is an import swap. New: the field
// wires its control itself (below), so a caller cannot leave `aria-describedby` or
// `aria-invalid` off.
//
// One labelled control, with its help text or its error underneath.
//
// **Error OR hint.** Two lines of small print under a field that is already wrong is two things
// to read before fixing one; the hint's job was to prevent the error that has now happened.
//
// **The wiring.** Given an id and a single native control (`<input>`, `<select>`, `<textarea>`),
// the field sets the control's `id`, points its `aria-describedby` at the message while there is
// one (keeping any ids the control already names), and sets `aria-invalid` while there is an
// error. A component control takes the same three as a function child,
// `{(control) => <SelectWithOther id={control.id} describedBy={control["aria-describedby"]} … />}`,
// or wires them by hand with `describedBy(id)`. A composite control with no single element to
// name (a people picker) passes no id: its label is a <span>, as in the reference.

import { cloneElement, isValidElement, type ReactElement, type ReactNode } from "react";
import { cx } from "../cx";
import { ui } from "../recipes";

/** What a field hands its control. */
export interface FieldControlProps {
  id?: string;
  "aria-describedby"?: string;
  "aria-invalid"?: true;
}

export interface FieldProps {
  label: string;
  /** Beside the label, outside the <label> element: a hint trigger is focusable. */
  labelHint?: ReactNode;
  /** The control's id (warehouse's name). A composite control passes none. */
  htmlFor?: string;
  /** The control's id (the reference's name for `htmlFor`). */
  id?: string;
  /** What to say before anything has gone wrong. Dropped once something has. */
  hint?: ReactNode;
  error?: ReactNode;
  /** The control, or a function that takes the field's wiring and returns it. */
  children: ReactNode | ((control: FieldControlProps) => ReactNode);
  className?: string;
}

/** The id of a field's message, for the control's `aria-describedby`. */
export function describedBy(id: string): string {
  return `${id}-msg`;
}

/** Space-separated ids, each once, or undefined when there are none. */
function joinIds(...lists: (string | undefined)[]): string | undefined {
  const ids = [...new Set(lists.flatMap((l) => (l ? l.split(/\s+/).filter(Boolean) : [])))];
  return ids.length > 0 ? ids.join(" ") : undefined;
}

type NativeControl = ReactElement<FieldControlProps & { id?: string }>;

/** The elements a <label> can name (HTML's labelable elements). Anything else, a wrapper around
 *  an input and its unit, is left as it is: wiring it would give two elements one id and point
 *  the label at the wrapper (Quadrogen Command Center, 2.31.0). */
const LABELABLE = new Set(["input", "select", "textarea", "button", "meter", "output", "progress"]);

function wire(children: FieldProps["children"], control: FieldControlProps): ReactNode {
  if (typeof children === "function") return children(control);
  if (!isValidElement(children) || typeof children.type !== "string") return children;
  if (!LABELABLE.has(children.type)) return children;
  const el = children as NativeControl;
  return cloneElement(el, {
    id: el.props.id ?? control.id,
    "aria-describedby": joinIds(el.props["aria-describedby"], control["aria-describedby"]),
    "aria-invalid": el.props["aria-invalid"] ?? control["aria-invalid"]
  });
}

export function Field({
  label,
  labelHint,
  htmlFor,
  id,
  hint,
  error,
  children,
  className
}: FieldProps) {
  const controlId = htmlFor ?? id;
  const msgId = controlId ? describedBy(controlId) : undefined;
  const hasError = error !== undefined && error !== null && error !== false && error !== "";
  const hasHint = !hasError && hint !== undefined && hint !== null && hint !== false && hint !== "";
  const control: FieldControlProps = {
    id: controlId,
    "aria-describedby": hasError || hasHint ? msgId : undefined,
    "aria-invalid": hasError ? true : undefined
  };
  return (
    <div className={cx("oxy-ui-form-field", className)}>
      <div className='oxy-ui-form-field__label-row'>
        {controlId ? (
          <label className={ui.label} htmlFor={controlId}>
            {label}
          </label>
        ) : (
          <span className={ui.label}>{label}</span>
        )}
        {labelHint}
      </div>
      {wire(children, control)}
      {hasError ? (
        <p id={msgId} role='alert' className='oxy-ui-form-field__error'>
          {error}
        </p>
      ) : hasHint ? (
        <div id={msgId} className={ui.note}>
          {hint}
        </div>
      ) : null}
    </div>
  );
}
