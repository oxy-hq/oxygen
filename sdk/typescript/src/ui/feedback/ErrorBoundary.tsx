// Started from pokehouse-oxy `apps/store-ops/src/chrome/ErrorBoundary.tsx` (BEYOND-GALLERY row 12:
// the one that reports and offers a way out; quadrogen's renders a line). Kept: the class (React
// has no hook for this), `fallback(error)`, the console left to React (it logs a caught error by
// itself). New: the fallback gets a `reset`, the boundary starts over when `resetKeys` change (a
// new route, another record), `onError` reports, and without a fallback it draws a `LoadError`
// whose Try again resets it, so focus is handed on to what renders next (decision 14). Left in
// Store Ops: its stale-chunk and offline wording (`ProblemCard`), which is its own deploy story.
//
// What a render that throws leaves on screen. Without a boundary React unmounts the whole root on
// a throw, and a page with no message and no control is what a person is left with. Put one
// around the whole app, and one around each part that can fail alone (a module, a panel), so one
// part's throw loses that part and not the page.

import { Component, type ErrorInfo, type ReactNode } from "react";
import { LoadError } from "./LoadError";

export interface ErrorBoundaryProps {
  /** What shows instead. Default: an error callout with Try again, drawn as `LoadError`. */
  fallback?: (error: unknown, reset: () => void) => ReactNode;
  /** Told of every caught error: send it to the app's report. */
  onError?: (error: unknown, info: ErrorInfo) => void;
  /** The boundary starts over when any of these change. */
  resetKeys?: readonly unknown[];
  /** The default fallback's claim. Default "This part stopped working". */
  title?: ReactNode;
  /** The default fallback's detail. Default: what to try. */
  detail?: ReactNode;
  children: ReactNode;
}

interface ErrorBoundaryState {
  failed: boolean;
  error: unknown;
}

const changed = (a: readonly unknown[] = [], b: readonly unknown[] = []) =>
  a.length !== b.length || a.some((v, i) => !Object.is(v, b[i]));

export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { failed: false, error: null };

  static getDerivedStateFromError(error: unknown): ErrorBoundaryState {
    return { failed: true, error };
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    this.props.onError?.(error, info);
  }

  componentDidUpdate(prev: ErrorBoundaryProps) {
    if (this.state.failed && changed(prev.resetKeys, this.props.resetKeys)) this.reset();
  }

  reset = () => {
    this.setState({ failed: false, error: null });
  };

  render() {
    if (!this.state.failed) return this.props.children;
    const { fallback, title, detail } = this.props;
    if (fallback) return fallback(this.state.error, this.reset);
    return (
      <LoadError title={title ?? "This part stopped working"} onRetry={this.reset}>
        {detail ?? "Try again, or reload the page. Anything already saved is kept."}
      </LoadError>
    );
  }
}
