import { Component, useEffect, type ErrorInfo, type ReactNode } from 'react'
import { isRouteErrorResponse, useNavigate, useRouteError } from 'react-router-dom'
import { AlertTriangle, Copy, RefreshCw } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import { Button } from '@/components/ui'

/**
 * Crash handling for the webview.
 *
 * Without a boundary, a single render error blanks the whole window — one
 * mistake in a dialog took out the entire app — and, worse, leaves no trace in
 * the application log. Everything the backend does is recorded; a frontend
 * crash was the one event that vanished silently. These report to the log
 * under the `ui` category before rendering anything.
 */

/** Anything can be thrown; get something worth reading out of it. */
export function describeError(error: unknown): { message: string; stack: string | null } {
  if (error instanceof Error) {
    return { message: error.message || error.name, stack: error.stack ?? null }
  }
  if (isRouteErrorResponse(error)) {
    return {
      message: `${error.status} ${error.statusText}`,
      stack: typeof error.data === 'string' ? error.data : null,
    }
  }
  if (typeof error === 'object' && error !== null && 'message' in error) {
    return { message: String((error as { message: unknown }).message), stack: null }
  }
  return { message: String(error), stack: null }
}

/** One block covering what broke and where, for pasting into a report. */
export function crashReport(
  scope: string,
  error: unknown,
  componentStack?: string | null,
): string {
  const { message, stack } = describeError(error)
  return [
    `pontifex crashed in ${scope}`,
    `error: ${message}`,
    stack ? `\nstack:\n${stack}` : '',
    componentStack ? `\ncomponents:${componentStack}` : '',
  ]
    .filter(Boolean)
    .join('\n')
}

function CrashPanel({
  scope,
  error,
  componentStack,
  onRetry,
}: {
  scope: string
  error: unknown
  componentStack?: string | null
  onRetry?: () => void
}) {
  const { message, stack } = describeError(error)

  return (
    <div className="flex h-full min-h-0 flex-col items-center justify-center gap-4 overflow-auto p-8">
      <AlertTriangle className="size-8 text-danger" />

      <div className="max-w-xl text-center">
        <h2 className="text-sm font-semibold text-ink">Something went wrong</h2>
        <p className="mt-1 text-xs text-ink-muted">
          {scope} stopped rendering. The rest of the app is still running, and
          this has been written to the application log.
        </p>
      </div>

      <p className="max-w-2xl rounded-md border border-danger/40 bg-danger/10 px-3 py-2 text-center font-mono text-[11px] text-danger">
        {message}
      </p>

      {(stack || componentStack) && (
        <details className="w-full max-w-2xl">
          <summary className="cursor-pointer text-[11px] text-ink-faint hover:text-ink-muted">
            Details
          </summary>
          <pre className="mt-2 max-h-64 overflow-auto rounded-md border border-edge bg-surface-0 p-2 font-mono text-[10px] leading-relaxed text-ink-faint">
            {[stack, componentStack].filter(Boolean).join('\n')}
          </pre>
        </details>
      )}

      <div className="flex items-center gap-2">
        {onRetry && (
          <Button variant="secondary" onClick={onRetry}>
            Try again
          </Button>
        )}
        <Button
          variant="ghost"
          onClick={() =>
            void navigator.clipboard.writeText(crashReport(scope, error, componentStack))
          }
        >
          <Copy className="size-3" />
          Copy details
        </Button>
        <Button variant="ghost" onClick={() => window.location.reload()}>
          <RefreshCw className="size-3" />
          Reload
        </Button>
      </div>
    </div>
  )
}

interface BoundaryProps {
  /** What this boundary wraps, e.g. "The Schemas screen". Used in the log. */
  scope: string
  children: ReactNode
}

interface BoundaryState {
  error: unknown
  componentStack: string | null
}

/**
 * Catches render errors below it.
 *
 * A class because `componentDidCatch` has no hook equivalent. Wraps the whole
 * app, so a crash in a provider — above the router, where `errorElement`
 * cannot reach — still shows something.
 */
export class ErrorBoundary extends Component<BoundaryProps, BoundaryState> {
  state: BoundaryState = { error: null, componentStack: null }

  static getDerivedStateFromError(error: unknown): Partial<BoundaryState> {
    return { error }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    this.setState({ componentStack: info.componentStack ?? null })
    // Fire-and-forget, and never throws — logging a crash must not cause one.
    ipc.log('error', 'ui', crashReport(this.props.scope, error, info.componentStack))
  }

  render() {
    if (this.state.error === null) return this.props.children
    return (
      <CrashPanel
        scope={this.props.scope}
        error={this.state.error}
        componentStack={this.state.componentStack}
        onRetry={() => this.setState({ error: null, componentStack: null })}
      />
    )
  }
}

/**
 * Route-level handler, given to each route as `errorElement`.
 *
 * Scoped to the outlet, so a crashing page leaves the nav and the environment
 * badge usable — you can walk to another screen instead of reloading.
 */
export function RouteErrorBoundary({ scope = 'This screen' }: { scope?: string }) {
  const error = useRouteError()
  const navigate = useNavigate()

  useEffect(() => {
    ipc.log('error', 'ui', crashReport(scope, error))
  }, [error, scope])

  return (
    <CrashPanel scope={scope} error={error} onRetry={() => navigate('/schemas')} />
  )
}
