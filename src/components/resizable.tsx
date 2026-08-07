import { Group, Panel, Separator, type Layout } from 'react-resizable-panels'
import { useSettings } from '@/app/settings-context'
import { cn } from './ui'

/**
 * Panel layout with sizes persisted in app settings.
 *
 * A dense three-pane editor is only usable if the panes fit the work — a wide
 * tree for a deep schema, a wide inspector for a long description. Sizes are
 * saved so that choice survives a restart.
 *
 * Layouts are a map of panel id to flex-grow, which we flatten to an ordered
 * array for storage using the panel ids given here.
 */
export function ResizableGroup({
  id,
  panelIds,
  orientation = 'horizontal',
  children,
  className,
}: {
  id: string
  /** Panel ids in order, so the stored array is stable across renders. */
  panelIds: string[]
  orientation?: 'horizontal' | 'vertical'
  children: React.ReactNode
  className?: string
}) {
  const { settings, savePanelSizes } = useSettings()
  const stored = settings?.panelSizes?.[id]

  const defaultLayout: Layout | undefined =
    stored && stored.length === panelIds.length
      ? Object.fromEntries(panelIds.map((panelId, i) => [panelId, stored[i]]))
      : undefined

  return (
    <Group
      id={id}
      orientation={orientation}
      className={cn('min-h-0', className)}
      defaultLayout={defaultLayout}
      onLayoutChanged={(layout) => {
        if (!settings) return
        const sizes = panelIds.map((panelId) => layout[panelId] ?? 0)
        const current = settings.panelSizes?.[id]
        // Only write when the layout actually moved; the callback also fires
        // on mount and would otherwise save on every navigation.
        if (
          current &&
          current.length === sizes.length &&
          current.every((v, i) => Math.abs(v - sizes[i]) < 0.001)
        ) {
          return
        }
        // Patches only this group's entry, so two panel groups saving at once
        // cannot clobber each other's sizes.
        savePanelSizes(id, sizes)
      }}
    >
      {children}
    </Group>
  )
}

export { Panel as ResizablePanel }

/** A drag handle that is easy to hit without stealing visual weight. */
export function ResizeHandle({
  orientation = 'vertical',
}: {
  /** `vertical` = a vertical bar between side-by-side panes. */
  orientation?: 'vertical' | 'horizontal'
}) {
  return (
    <Separator
      className={cn(
        'relative shrink-0 bg-edge transition-colors',
        'hover:bg-edge-strong data-[separator-dragging]:bg-accent',
        orientation === 'vertical'
          ? 'w-px cursor-col-resize'
          : 'h-px cursor-row-resize',
      )}
      style={{ outline: '2px solid transparent', outlineOffset: '2px' }}
    />
  )
}
