import { clsx, type ClassValue } from 'clsx'
import { twMerge } from 'tailwind-merge'
import * as Dialog from '@radix-ui/react-dialog'
import { openUrl } from '@tauri-apps/plugin-opener'
import * as SelectMenuPrimitive from '@radix-ui/react-select'
import type { ReactNode } from 'react'
import { useState } from 'react'
import {
  AlertTriangle,
  Check,
  CheckCircle2,
  ChevronsUpDown,
  Copy,
  Info,
  Loader2,
  XCircle,
} from 'lucide-react'
import type { Finding, IpcError, Severity } from '@/lib/types'

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

// --- buttons --------------------------------------------------------------

type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger'

const buttonStyles: Record<ButtonVariant, string> = {
  primary:
    'bg-accent text-on-accent hover:bg-accent/90 font-medium disabled:bg-accent/40',
  secondary:
    'bg-surface-2 text-ink hover:bg-surface-3 border border-edge disabled:text-ink-faint',
  ghost: 'text-ink-muted hover:text-ink hover:bg-surface-2',
  danger:
    'bg-danger/15 text-danger border border-danger/40 hover:bg-danger/25 disabled:opacity-40',
}

export function Button({
  variant = 'secondary',
  size = 'md',
  loading = false,
  className,
  children,
  disabled,
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant
  size?: 'sm' | 'md'
  loading?: boolean
}) {
  return (
    <button
      {...props}
      disabled={disabled || loading}
      className={cn(
        'inline-flex items-center justify-center gap-1.5 rounded-md whitespace-nowrap',
        'transition-colors disabled:cursor-not-allowed',
        size === 'sm' ? 'h-6 px-2 text-[11px]' : 'h-7 px-2.5 text-xs',
        buttonStyles[variant],
        className,
      )}
    >
      {loading && <Loader2 className="size-3 animate-spin" />}
      {children}
    </button>
  )
}

// --- form controls --------------------------------------------------------

export function Input({
  className,
  ...props
}: React.InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      {...props}
      className={cn(
        'h-7 w-full rounded-md border border-edge bg-surface-1 px-2 text-xs text-ink',
        'placeholder:text-ink-faint focus:border-accent focus:outline-none',
        'disabled:text-ink-faint',
        className,
      )}
    />
  )
}

export function Textarea({
  className,
  ...props
}: React.TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      {...props}
      className={cn(
        'w-full resize-y rounded-md border border-edge bg-surface-1 px-2 py-1.5 text-xs leading-snug text-ink',
        'placeholder:text-ink-faint focus:border-accent focus:outline-none',
        'disabled:text-ink-faint',
        className,
      )}
    />
  )
}

/**
 * A list long enough to deserve the app's own styling.
 *
 * The native `<select>` below is right for three options — it is one element,
 * it behaves the way the platform does, and nobody wants a hand-rolled
 * dropdown for "= or ≠". It is wrong for forty Jira projects: macOS draws its
 * own menu in its own colours, as tall as the list, over everything.
 *
 * This one is the app's colours, ten rows tall, and scrolls past that.
 */
export function SelectMenu({
  value,
  onChange,
  options,
  placeholder,
  className,
  disabled,
}: {
  value: string
  onChange: (value: string) => void
  options: readonly { value: string; label: string }[]
  placeholder?: string
  className?: string
  disabled?: boolean
}) {
  return (
    <SelectMenuPrimitive.Root
      // Radix refuses an empty string as a value, so "nothing chosen" is the
      // absence of one — which is also what shows the placeholder.
      value={value || undefined}
      onValueChange={onChange}
      disabled={disabled}
    >
      <SelectMenuPrimitive.Trigger
        className={cn(
          'flex h-7 w-full items-center gap-1 rounded-md border border-edge bg-surface-1 px-2 text-xs text-ink',
          'focus:border-accent focus:outline-none disabled:text-ink-faint',
          className,
        )}
      >
        <span className="min-w-0 flex-1 truncate text-left">
          <SelectMenuPrimitive.Value placeholder={placeholder} />
        </span>
        <SelectMenuPrimitive.Icon>
          <ChevronsUpDown className="size-3 shrink-0 text-ink-faint" />
        </SelectMenuPrimitive.Icon>
      </SelectMenuPrimitive.Trigger>

      <SelectMenuPrimitive.Portal>
        <SelectMenuPrimitive.Content
          // Anchored to the trigger rather than covering it, and never wider
          // than it needs to be — a project list is read left to right.
          position="popper"
          sideOffset={4}
          // Not `panel`: that is the plate style, and it carries a margin —
          // which on something anchored to a trigger is an offset.
          className="z-50 overflow-hidden rounded-md border border-edge bg-surface-1 shadow-2xl"
        >
          {/* Ten rows, then a scrollbar. `min-w` keeps a short list from
              drawing a menu narrower than the control it came from. */}
          <SelectMenuPrimitive.Viewport className="max-h-[15rem] min-w-[var(--radix-select-trigger-width)] overflow-y-auto p-1">
            {options.map((option) => (
              <SelectMenuPrimitive.Item
                key={option.value}
                value={option.value}
                className={cn(
                  'flex cursor-pointer items-center gap-1.5 rounded px-1.5 py-1 text-xs text-ink-muted outline-none',
                  'data-[highlighted]:bg-surface-3 data-[highlighted]:text-ink',
                  'data-[state=checked]:text-accent',
                )}
              >
                <SelectMenuPrimitive.ItemIndicator className="shrink-0">
                  <Check className="size-3" />
                </SelectMenuPrimitive.ItemIndicator>
                <SelectMenuPrimitive.ItemText>{option.label}</SelectMenuPrimitive.ItemText>
              </SelectMenuPrimitive.Item>
            ))}
          </SelectMenuPrimitive.Viewport>
        </SelectMenuPrimitive.Content>
      </SelectMenuPrimitive.Portal>
    </SelectMenuPrimitive.Root>
  )
}

export function Select({
  className,
  children,
  ...props
}: React.SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select
      {...props}
      className={cn(
        'h-7 rounded-md border border-edge bg-surface-1 px-1.5 text-xs text-ink',
        'focus:border-accent focus:outline-none disabled:text-ink-faint',
        className,
      )}
    >
      {children}
    </select>
  )
}

export function Field({
  label,
  hint,
  children,
  className,
}: {
  label: string
  hint?: ReactNode
  children: ReactNode
  className?: string
}) {
  return (
    <label className={cn('flex flex-col gap-1', className)}>
      <span className="text-[11px] font-medium text-ink-muted">{label}</span>
      {children}
      {hint && <span className="text-[10px] text-ink-faint">{hint}</span>}
    </label>
  )
}

/**
 * A checkbox drawn from the theme's tokens.
 *
 * The native control takes `accent-color` and nothing else: under a dark
 * colour scheme WebKit paints the unchecked box black whatever the surface
 * is, which on blueprint paper looked like a hole. The input stays in the
 * tree for behaviour, focus and assistive tech; the box beside it is what
 * you see.
 */
export function Checkbox({
  label,
  className,
  title,
  ...props
}: React.InputHTMLAttributes<HTMLInputElement> & { label: ReactNode }) {
  return (
    <label
      title={title}
      className={cn(
        'inline-flex cursor-pointer items-center gap-1.5 text-xs text-ink-muted',
        props.disabled && 'cursor-not-allowed opacity-50',
        className,
      )}
    >
      <input type="checkbox" {...props} className="peer sr-only" />
      <span
        aria-hidden
        className={cn(
          'flex size-3.5 shrink-0 items-center justify-center rounded-sm border border-edge-strong bg-surface-1 transition-colors',
          'peer-checked:border-accent peer-checked:bg-accent peer-checked:[&>svg]:opacity-100',
          'peer-focus-visible:outline peer-focus-visible:outline-2 peer-focus-visible:outline-offset-1 peer-focus-visible:outline-accent',
        )}
      >
        <Check className="size-2.5 text-on-accent opacity-0" strokeWidth={3} />
      </span>
      {label}
    </label>
  )
}

// --- layout ---------------------------------------------------------------

export function Panel({
  title,
  actions,
  children,
  className,
  bodyClassName,
}: {
  title?: ReactNode
  actions?: ReactNode
  children: ReactNode
  className?: string
  bodyClassName?: string
}) {
  return (
    <section
      className={cn(
        'panel flex min-h-0 flex-col overflow-hidden rounded-lg bg-surface-1',
        className,
      )}
    >
      {(title || actions) && (
        <header className="chrome-panel flex h-9 shrink-0 items-center justify-between gap-2 border-b border-edge px-3">
          <h2 className="truncate text-xs font-semibold text-ink">{title}</h2>
          {actions && <div className="flex shrink-0 items-center gap-1.5">{actions}</div>}
        </header>
      )}
      <div className={cn('min-h-0 flex-1 overflow-auto', bodyClassName)}>
        {children}
      </div>
    </section>
  )
}

/**
 * The shell every dialog in the app shares: overlay, centring, and chrome.
 *
 * Seven dialogs had each spelled this out, so the overlay tint and the border
 * radius were seven copies of the same string. Body layout stays with the
 * caller — some dialogs are a padded box, others a scrolling flex column —
 * which is the part that genuinely differs.
 */
export function Modal({
  open = true,
  onClose,
  width,
  tone = 'default',
  className,
  children,
}: {
  open?: boolean
  onClose: () => void
  /** Content width in pixels. */
  width: number
  tone?: 'default' | 'danger'
  /** Layout for the content box, e.g. `p-4` or `flex max-h-[80vh] flex-col`. */
  className?: string
  children: ReactNode
}) {
  return (
    <Dialog.Root open={open} onOpenChange={(next) => !next && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-overlay" />
        <Dialog.Content
          style={{ width }}
          className={cn(
            // `m-0`: a plate margin would nudge a centred dialog off centre.
            'panel m-0 fixed left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 rounded-lg bg-surface-1 shadow-2xl',
            tone === 'danger' && 'border-danger/50',
            className,
          )}
        >
          {children}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

export function ModalTitle({
  children,
  tone = 'default',
}: {
  children: ReactNode
  tone?: 'default' | 'danger'
}) {
  return (
    <Dialog.Title
      className={cn(
        'flex items-center gap-2 text-sm font-semibold',
        tone === 'danger' ? 'text-danger' : 'text-ink',
      )}
    >
      {children}
    </Dialog.Title>
  )
}

export function ModalDescription({
  children,
  className,
}: {
  children: ReactNode
  className?: string
}) {
  return (
    <Dialog.Description className={cn('mt-1 text-xs text-ink-muted', className)}>
      {children}
    </Dialog.Description>
  )
}

/**
 * A slider over a fixed list of choices.
 *
 * The underlying range is the *index*, not the value, so unevenly spaced steps
 * (15s, 30s, 60s, 90s, 120s) get equal travel — dragging feels linear while the
 * values grow the way you actually reason about a time budget.
 */
export function StepSlider<T extends number | string>({
  value,
  steps,
  onChange,
  label,
  format = (v) => String(v),
  className,
}: {
  value: T
  steps: readonly T[]
  onChange: (value: T) => void
  label?: string
  format?: (value: T) => string
  className?: string
}) {
  // An unrecognised value would send the thumb to 0 and silently re-pick the
  // first step; clamp to the nearest end instead.
  const index = Math.max(0, steps.indexOf(value))
  return (
    <label className={cn('flex items-center gap-2', className)}>
      {label && <span className="shrink-0 text-[10px] text-ink-faint">{label}</span>}
      <input
        type="range"
        min={0}
        max={steps.length - 1}
        step={1}
        value={index}
        onChange={(e) => onChange(steps[Number(e.target.value)])}
        className="h-1 w-24 cursor-pointer appearance-none rounded-full bg-surface-3 accent-accent"
        aria-label={label}
      />
      <span className="w-10 shrink-0 text-right font-mono text-[10px] text-ink-muted">
        {format(value)}
      </span>
    </label>
  )
}

/**
 * A row of mutually exclusive buttons — the app's view/mode switcher.
 *
 * Five screens had hand-rolled this and they had already drifted: only two
 * carried the disabled styling, so a disabled segment elsewhere looked
 * clickable.
 */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
  size = 'md',
  className,
}: {
  value: T
  options: readonly { id: T; label: string; disabled?: boolean; title?: string }[]
  onChange: (id: T) => void
  size?: 'sm' | 'md' | 'lg'
  className?: string
}) {
  const sizes = {
    sm: 'h-5 px-2 text-[10px]',
    md: 'h-6 px-2 text-[11px]',
    lg: 'h-7 flex-1 text-[11px]',
  }
  return (
    <div className={cn('flex rounded-md border border-edge', className)}>
      {options.map((option) => (
        <button
          key={option.id}
          type="button"
          onClick={() => onChange(option.id)}
          disabled={option.disabled}
          title={option.title}
          className={cn(
            sizes[size],
            'first:rounded-l-md last:rounded-r-md',
            option.id === value
              ? 'bg-surface-3 text-ink'
              : 'text-ink-muted hover:bg-surface-2 disabled:text-ink-faint disabled:hover:bg-transparent',
          )}
        >
          {option.label}
        </button>
      ))}
    </div>
  )
}

export function Toolbar({
  children,
  className,
}: {
  children: ReactNode
  className?: string
}) {
  return (
    <div
      className={cn(
        'chrome-page flex shrink-0 flex-wrap items-center gap-2 border-b border-edge bg-surface-1 px-3 py-2',
        className,
      )}
    >
      {children}
    </div>
  )
}

// --- status ---------------------------------------------------------------

type BadgeTone = 'neutral' | 'ok' | 'warn' | 'danger' | 'info' | 'accent'

const badgeTones: Record<BadgeTone, string> = {
  neutral: 'bg-surface-3 text-ink-muted',
  ok: 'bg-ok/15 text-ok',
  warn: 'bg-warn/15 text-warn',
  danger: 'bg-danger/15 text-danger',
  info: 'bg-info/15 text-info',
  accent: 'bg-accent/15 text-accent',
}

export function Badge({
  tone = 'neutral',
  children,
  className,
  title,
}: {
  tone?: BadgeTone
  children: ReactNode
  className?: string
  title?: string
}) {
  return (
    <span
      title={title}
      className={cn(
        // Never wraps: a badge is a single token, and "200 sampled" broken
        // across two lines in a narrow toolbar reads as two separate facts.
        'inline-flex shrink-0 items-center gap-1 whitespace-nowrap rounded px-1.5 py-0.5 text-[10px] font-medium',
        badgeTones[tone],
        className,
      )}
    >
      {children}
    </span>
  )
}

/**
 * A link to somewhere outside the app.
 *
 * `<a target="_blank">` opens nothing here: a webview has no tab to open one
 * in, and no browser behind it. So the href is never followed — it is there
 * for what a screen reader announces and what the right-click menu offers —
 * and the click hands the URL to the opener plugin, which gives it to the
 * system browser, the same way the repo links and the sign-in flow do.
 *
 * Clicks stop here, because these sit inside rows that navigate.
 */
export function OpenLink({
  url,
  className,
  title,
  children,
}: {
  url: string
  className?: string
  title?: string
  children: ReactNode
}) {
  return (
    <a
      href={url}
      title={title ?? url}
      onClick={(event) => {
        event.preventDefault()
        event.stopPropagation()
        void openUrl(url)
      }}
      className={className}
    >
      {children}
    </a>
  )
}

/**
 * Render `backticked` spans as code.
 *
 * Analysis summaries are written as sentences on the Rust side — the same text
 * that becomes a ticket title — so field names in them are marked the way
 * plain text marks them rather than pre-split into React nodes.
 */
export function Marked({ text }: { text: string }) {
  return (
    <>
      {text.split(/`([^`]+)`/).map((part, i) =>
        i % 2 === 1 ? (
          <span key={i} className="font-mono text-ink">
            {part}
          </span>
        ) : (
          part
        ),
      )}
    </>
  )
}

/**
 * Copy `text` to the clipboard, with a moment of confirmation.
 *
 * Stops the click from reaching the row behind it, since the rows it sits in
 * toggle open on click and copying should not do that.
 */
export function CopyButton({ text, title = 'Copy' }: { text: string; title?: string }) {
  const [copied, setCopied] = useState(false)
  return (
    <Button
      variant="ghost"
      size="sm"
      title={title}
      onClick={(e) => {
        e.stopPropagation()
        void navigator.clipboard.writeText(text).then(() => {
          setCopied(true)
          setTimeout(() => setCopied(false), 1500)
        })
      }}
    >
      {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
    </Button>
  )
}

export function Spinner({ label }: { label?: string }) {
  return (
    <div className="flex items-center justify-center gap-2 p-6 text-xs text-ink-faint">
      <Loader2 className="size-4 animate-spin" />
      {label}
    </div>
  )
}

export function EmptyState({
  icon,
  title,
  detail,
  action,
}: {
  icon?: ReactNode
  title: string
  detail?: ReactNode
  action?: ReactNode
}) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 p-8 text-center">
      {icon && <div className="text-ink-faint">{icon}</div>}
      <p className="text-sm text-ink-muted">{title}</p>
      {detail && <p className="max-w-md text-xs text-ink-faint">{detail}</p>}
      {action && <div className="mt-2">{action}</div>}
    </div>
  )
}

/**
 * Renders an IPC failure. Auth errors get a sign-in affordance, because the
 * fix is a specific action rather than a retry.
 *
 * When `onLogin` is absent on an auth error, the profile's credentials come
 * from outside pontifex — an external manager like Leapp or aws-vault wrote them
 * into `~/.aws/credentials`. Offering a sign-in there would be a button that
 * cannot work, so we say where the fix actually lives instead.
 */
export function ErrorBox({
  error,
  onLogin,
  hint,
  className,
}: {
  error: IpcError
  onLogin?: () => void
  /** What to do instead, when signing in from here would not help. */
  hint?: string
  className?: string
}) {
  const isAuth = error.kind === 'auth'
  return (
    <div
      className={cn(
        'flex items-start gap-2 rounded-md border px-3 py-2 text-xs',
        isAuth
          ? 'border-warn/40 bg-warn/10 text-warn'
          : 'border-danger/40 bg-danger/10 text-danger',
        className,
      )}
    >
      {isAuth ? (
        <AlertTriangle className="mt-px size-3.5 shrink-0" />
      ) : (
        <XCircle className="mt-px size-3.5 shrink-0" />
      )}
      <div className="min-w-0 flex-1">
        <p className="break-words">{error.message}</p>
        {isAuth &&
          (onLogin ? (
            <Button
              variant="secondary"
              size="sm"
              className="mt-2"
              onClick={onLogin}
            >
              Sign in
            </Button>
          ) : (
            hint && <p className="mt-1.5 text-ink-muted">{hint}</p>
          ))}
      </div>
    </div>
  )
}

/** Callout styling per tone, so the app has one definition of "a callout". */
const noteTones = {
  info: { box: 'border-info/30 bg-info/10 text-info', icon: Info },
  warn: { box: 'border-warn/40 bg-warn/10 text-warn', icon: AlertTriangle },
  danger: { box: 'border-danger/40 bg-danger/10 text-danger', icon: AlertTriangle },
  ok: { box: 'border-ok/40 bg-ok/10 text-ok', icon: CheckCircle2 },
} as const

export function Note({
  tone = 'info',
  children,
  className,
}: {
  tone?: keyof typeof noteTones
  children: ReactNode
  className?: string
}) {
  const { box, icon: Icon } = noteTones[tone]
  return (
    <div
      className={cn(
        'flex items-start gap-2 rounded-md border px-3 py-2 text-xs',
        box,
        className,
      )}
    >
      <Icon className="mt-px size-3.5 shrink-0" />
      <div className="min-w-0 flex-1">{children}</div>
    </div>
  )
}

/** Validation findings, grouped so errors read before warnings. */
export function FindingList({
  findings,
  onSelect,
  className,
}: {
  findings: Finding[]
  onSelect?: (finding: Finding) => void
  className?: string
}) {
  if (findings.length === 0) return null

  const order: Record<Severity, number> = { error: 0, warning: 1 }
  const sorted = [...findings].sort(
    (a, b) => order[a.severity] - order[b.severity],
  )

  return (
    <ul className={cn('flex flex-col gap-1', className)}>
      {sorted.map((finding, index) => (
        <li key={`${finding.path}-${index}`}>
          <button
            type="button"
            onClick={() => onSelect?.(finding)}
            className={cn(
              'flex w-full items-start gap-2 rounded px-2 py-1 text-left text-[11px]',
              onSelect && 'hover:bg-surface-2',
              finding.severity === 'error' ? 'text-danger' : 'text-warn',
            )}
          >
            {finding.severity === 'error' ? (
              <XCircle className="mt-px size-3 shrink-0" />
            ) : (
              <AlertTriangle className="mt-px size-3 shrink-0" />
            )}
            <span className="min-w-0 flex-1 break-words">
              {finding.message}
              {finding.path && (
                <span className="ml-1 font-mono text-ink-faint">
                  {finding.path}
                </span>
              )}
            </span>
          </button>
        </li>
      ))}
    </ul>
  )
}
