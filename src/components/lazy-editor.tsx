import { Suspense, lazy, type ComponentProps } from 'react'
import { Spinner } from '@/components/ui'
// Type-only: erased at build time, so this does not pull Monaco into the graph.
import type { JsonDiff as JsonDiffImpl, JsonEditor as JsonEditorImpl } from './editor'

/**
 * Monaco, loaded on demand.
 *
 * Monaco is 3.9 MB of JavaScript plus 156 KB of CSS, and importing it
 * statically anywhere put it in the entry chunk — parsed and executed before
 * React mounted, on every launch, including on Logs, Health, Topology,
 * Settings and Developer, none of which ever open an editor. Splitting the
 * chunk in `vite.config.ts` did not help: that splits the *file*, not the
 * import graph. Only a dynamic import defers the work.
 *
 * Import editors from here rather than from `./editor` directly, or the static
 * chain comes back.
 */
const LazyJsonEditor = lazy(() =>
  import('./editor').then((m) => ({ default: m.JsonEditor })),
)
const LazyJsonDiff = lazy(() => import('./editor').then((m) => ({ default: m.JsonDiff })))

export function JsonEditor(props: ComponentProps<typeof JsonEditorImpl>) {
  return (
    <Suspense fallback={<Spinner label="Loading editor…" />}>
      <LazyJsonEditor {...props} />
    </Suspense>
  )
}

export function JsonDiff(props: ComponentProps<typeof JsonDiffImpl>) {
  return (
    <Suspense fallback={<Spinner label="Loading editor…" />}>
      <LazyJsonDiff {...props} />
    </Suspense>
  )
}
