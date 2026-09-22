import { Check, Minus, Monitor, Plus } from 'lucide-react'
import { Button, Panel, cn } from '@/components/ui'
import { useSettings } from '@/app/settings-context'
import { DEFAULT_ZOOM, ZOOM_STEPS, formatZoom, stepZoom } from '@/app/zoom'
import { SYSTEM_THEME, THEMES, type ThemeDefinition } from './themes'
import { useTheme } from './theme-context'

/**
 * A miniature of the app drawn in a theme's own tokens.
 *
 * The card carries `data-theme`, and because the token mapping is inline every
 * utility inside resolves against that theme rather than the active one — so
 * this is the real palette, not a hand-picked swatch that can drift from it.
 */
function Preview({ theme }: { theme: ThemeDefinition }) {
  return (
    <div
      data-theme={theme.id}
      aria-hidden
      className="flex h-24 flex-col overflow-hidden rounded-md border border-edge bg-surface-0 text-ink"
    >
      <div className="chrome-app flex h-5 shrink-0 items-center gap-1 border-b border-edge bg-surface-1 px-1.5">
        <span className="h-2.5 w-8 rounded-sm bg-surface-3" />
        <span className="h-2.5 w-6 rounded-sm bg-surface-2" />
        <span className="ml-auto h-2.5 w-7 rounded-sm bg-ok/25" />
      </div>
      <div className="flex min-h-0 flex-1 gap-1.5 p-1.5">
        <div className="panel flex w-1/3 flex-col gap-1 rounded-sm bg-surface-1 p-1">
          <span className="h-1.5 w-3/4 rounded-sm bg-ink-muted/70" />
          <span className="h-1.5 w-1/2 rounded-sm bg-ink-faint/60" />
          <span className="h-1.5 w-2/3 rounded-sm bg-accent" />
          <span className="h-1.5 w-1/2 rounded-sm bg-ink-faint/60" />
        </div>
        <div className="panel flex flex-1 flex-col gap-1 rounded-sm bg-surface-1 p-1">
          <div className="flex items-center gap-1">
            <span className="h-2 w-8 rounded-sm bg-ink/80" />
            <span className="ml-auto h-3 w-8 rounded-sm bg-accent" />
          </div>
          <span className="h-1.5 w-full rounded-sm bg-type-object/50" />
          <span className="h-1.5 w-5/6 rounded-sm bg-type-string/50" />
          <span className="h-1.5 w-2/3 rounded-sm bg-type-number/50" />
          <div className="mt-auto flex gap-1">
            <span className="size-2 rounded-full bg-ok" />
            <span className="size-2 rounded-full bg-warn" />
            <span className="size-2 rounded-full bg-danger" />
            <span className="size-2 rounded-full bg-info" />
          </div>
        </div>
      </div>
    </div>
  )
}

function Card({
  selected,
  onSelect,
  title,
  description,
  children,
}: {
  selected: boolean
  onSelect: () => void
  title: string
  description: string
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      onClick={onSelect}
      className={cn(
        'flex w-52 flex-col gap-2 rounded-lg border p-2 text-left transition-colors',
        selected
          ? 'border-accent bg-accent/10'
          : 'border-edge hover:border-edge-strong hover:bg-surface-2',
      )}
    >
      {children}
      <div className="flex items-start gap-1.5 px-0.5">
        <div className="min-w-0 flex-1">
          <p className={cn('text-xs font-medium', selected ? 'text-accent' : 'text-ink')}>
            {title}
          </p>
          <p className="text-[10px] leading-snug text-ink-faint">{description}</p>
        </div>
        {selected && <Check className="mt-0.5 size-3.5 shrink-0 text-accent" />}
      </div>
    </button>
  )
}

/**
 * Choose a theme.
 *
 * Applies as soon as a card is clicked and is saved on its own, outside the
 * Settings page's draft-and-save flow: a look is something you try, and a
 * Save button between the click and the result would make trying it slow.
 */
export function ThemePicker() {
  const { preference, systemTheme, setPreference } = useTheme()
  return (
    <Panel title="Theme" bodyClassName="flex flex-col gap-2 p-3">
      <div role="radiogroup" aria-label="Theme" className="flex flex-wrap gap-3">
        {THEMES.map((t) => (
          <Card
            key={t.id}
            selected={preference === t.id}
            onSelect={() => setPreference(t.id)}
            title={t.label}
            description={t.description}
          >
            <Preview theme={t} />
          </Card>
        ))}
        <Card
          selected={preference === SYSTEM_THEME}
          onSelect={() => setPreference(SYSTEM_THEME)}
          title="System"
          description={`Light or dark, whichever the OS is using — ${systemTheme.label.toLowerCase()} right now.`}
        >
          <div className="relative">
            <Preview theme={systemTheme} />
            <Monitor className="absolute right-2 top-2 size-3.5 text-ink-faint" />
          </div>
        </Card>
      </div>
      <p className="px-0.5 text-[10px] text-ink-faint">
        Applies immediately and is remembered across restarts.
      </p>
    </Panel>
  )
}

/** The webview zoom, with the shortcuts spelled out beside it. */
export function TextSize() {
  const { zoom, setZoom } = useSettings()
  const atMin = zoom <= ZOOM_STEPS[0]
  const atMax = zoom >= ZOOM_STEPS[ZOOM_STEPS.length - 1]
  const modifier = navigator.platform.startsWith('Mac') ? '⌘' : 'Ctrl'
  return (
    <Panel title="Text size" bodyClassName="flex flex-col gap-2 p-3">
      <div className="flex items-center gap-2">
        <Button
          size="sm"
          disabled={atMin}
          onClick={() => setZoom(stepZoom(zoom, -1))}
          title={`Smaller (${modifier} −)`}
        >
          <Minus className="size-3" />
        </Button>
        <span className="w-12 text-center font-mono text-xs tabular-nums text-ink">
          {formatZoom(zoom)}
        </span>
        <Button
          size="sm"
          disabled={atMax}
          onClick={() => setZoom(stepZoom(zoom, 1))}
          title={`Larger (${modifier} +)`}
        >
          <Plus className="size-3" />
        </Button>
        <Button
          variant="ghost"
          size="sm"
          disabled={zoom === DEFAULT_ZOOM}
          onClick={() => setZoom(DEFAULT_ZOOM)}
          title={`Back to 100% (${modifier} 0)`}
        >
          Reset
        </Button>
      </div>
      <p className="px-0.5 text-[10px] text-ink-faint">
        {modifier} + and {modifier} − from any screen; {modifier} 0 resets. Remembered
        across restarts.
      </p>
    </Panel>
  )
}
