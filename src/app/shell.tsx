import { useQuery } from '@tanstack/react-query'
import { useEffect } from 'react'
import { NavLink, Outlet, useNavigate } from 'react-router-dom'
import {
  Braces,
  ClipboardList,
  GitFork,
  Radar,
  ScrollText,
  Settings as SettingsIcon,
  Sparkles,
  Wrench,
} from 'lucide-react'
import * as ipc from '@/lib/ipc'
import { Badge, Select, Spinner, cn } from '@/components/ui'
import { useSettings } from './settings-context'
import { useLoginForEnvironment } from './login-dialog'
import { useWatchEvents, useWatchStatuses } from '@/features/watch/use-watch-events'
import { IdleWatchGuard } from '@/features/watch/idle-guard'
import { applyZoom, zoomForKey } from './zoom'

const NAV = [
  { to: '/schemas', label: 'Schemas', icon: Braces },
  { to: '/generate', label: 'Generate', icon: Sparkles },
  { to: '/logs', label: 'Logs', icon: ScrollText },
  { to: '/watch', label: 'Watch', icon: Radar },
  { to: '/report', label: 'Health', icon: ClipboardList },
  { to: '/topology', label: 'Topology', icon: GitFork },
  { to: '/settings', label: 'Settings', icon: SettingsIcon },
]

/** Only shown when developer mode is on. */
const DEV_NAV = { to: '/developer', label: 'Developer', icon: Wrench }

/**
 * Live credential state for the active environment.
 *
 * Shown in the header because every screen depends on it — an expired token
 * makes the whole app fail, and this turns that from a wall of errors into one
 * clickable badge.
 */
function CredentialBadge() {
  const { activeEnvironment } = useSettings()
  const credentials = useLoginForEnvironment(activeEnvironment)
  const navigate = useNavigate()

  const { data, isLoading } = useQuery({
    queryKey: ['envCheck', activeEnvironment?.id],
    queryFn: () => ipc.checkEnvironment(activeEnvironment!.id),
    enabled: !!activeEnvironment,
    // Tokens expire on their own schedule; re-check periodically so the badge
    // does not go stale while the window sits open.
    refetchInterval: 5 * 60 * 1000,
    retry: false,
  })

  if (!activeEnvironment) return null
  if (isLoading) {
    return <Badge tone="neutral">checking…</Badge>
  }

  if (data?.ok) {
    return (
      <Badge tone="ok" title={data.identity?.arn ?? undefined}>
        {data.identity?.account ?? 'signed in'}
      </Badge>
    )
  }

  // Credentials that cannot be refreshed from here get a route to the actual
  // fix rather than a sign-in that would fail.
  if (data?.needsLogin && !credentials.onLogin) {
    return (
      <button
        type="button"
        onClick={() => navigate('/settings')}
        title={`${credentials.hint ?? ''} Click to open Settings.`.trim()}
      >
        <Badge tone="warn">fix credentials</Badge>
      </button>
    )
  }

  return (
    <button
      type="button"
      onClick={credentials.onLogin}
      title={data?.error?.message}
    >
      <Badge tone={data?.needsLogin ? 'warn' : 'danger'}>
        {data?.needsLogin ? 'sign in' : 'no access'}
      </Badge>
    </button>
  )
}

function EnvironmentPicker() {
  const { settings, activeEnvironment, setActiveEnvironment } = useSettings()
  if (!settings || settings.environments.length === 0) return null

  return (
    <Select
      value={activeEnvironment?.id ?? ''}
      onChange={(e) => setActiveEnvironment(e.target.value)}
      className={cn(
        'font-medium',
        activeEnvironment?.protected && 'border-danger/60 text-danger',
      )}
      title={
        activeEnvironment
          ? [
              activeEnvironment.sso
                ? `${activeEnvironment.sso.accountName ?? activeEnvironment.sso.accountId} · ${activeEnvironment.sso.roleName}`
                : activeEnvironment.awsProfile,
              activeEnvironment.region,
              activeEnvironment.registryName,
            ].join(' · ')
          : undefined
      }
    >
      {settings.environments.map((env) => (
        <option key={env.id} value={env.id}>
          {env.label}
        </option>
      ))}
    </Select>
  )
}

/**
 * Unread hits across every environment, for the nav badge.
 *
 * Every environment, not just the active one: a watch on prd should be able
 * to get your attention while you are working in dev.
 */
function useUnreadHits(): number {
  const { data } = useWatchStatuses()
  return data?.reduce((n, s) => n + s.unread, 0) ?? 0
}

/**
 * Text size: ⌘/Ctrl `=`, `-` and `0`.
 *
 * Applies the saved level once settings arrive, then listens for the
 * shortcuts. The zoom itself goes straight to the webview; the setting only
 * follows so it is there next launch.
 */
function useZoomHotkeys() {
  const { zoom, setZoom, isLoading } = useSettings()

  useEffect(() => {
    if (!isLoading) applyZoom(zoom)
  }, [zoom, isLoading])

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const next = zoomForKey(event, zoom)
      if (next === null) return
      event.preventDefault()
      if (next !== zoom) setZoom(next)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [zoom, setZoom])
}

export function Shell() {
  const { isLoading, activeEnvironment, settings } = useSettings()
  const nav = settings?.developerMode ? [...NAV, DEV_NAV] : NAV
  useWatchEvents()
  useZoomHotkeys()
  const unread = useUnreadHits()

  if (isLoading) {
    return (
      <div className="flex h-full items-center justify-center">
        <Spinner label="Loading settings…" />
      </div>
    )
  }

  return (
    <div className="flex h-full flex-col bg-surface-0">
      <header className="chrome-app flex h-11 shrink-0 items-center gap-3 border-b border-edge bg-surface-1 px-3">
        <nav className="flex items-center gap-0.5">
          {nav.map(({ to, label, icon: Icon }) => (
            <NavLink
              key={to}
              to={to}
              className={({ isActive }) =>
                cn(
                  'flex h-7 items-center gap-1.5 rounded-md px-2.5 text-xs transition-colors',
                  isActive
                    ? 'bg-surface-3 text-ink'
                    : 'text-ink-muted hover:bg-surface-2 hover:text-ink',
                )
              }
            >
              <Icon className="size-3.5" />
              {label}
              {to === '/watch' && unread > 0 && (
                <span
                  className="rounded-full bg-accent px-1.5 text-[10px] font-medium leading-4 text-on-accent"
                  title={`${unread} unread hit${unread === 1 ? '' : 's'}`}
                >
                  {unread > 99 ? '99+' : unread}
                </span>
              )}
            </NavLink>
          ))}
        </nav>

        <div className="ml-auto flex items-center gap-2">
          {activeEnvironment?.protected && (
            <Badge tone="danger" title="Destructive actions require extra confirmation">
              protected
            </Badge>
          )}
          <span className="font-mono text-[10px] text-ink-faint">
            {activeEnvironment?.registryName}
          </span>
          <EnvironmentPicker />
          <CredentialBadge />
        </div>
      </header>

      <main className="min-h-0 flex-1 overflow-hidden">
        <Outlet />
      </main>
      <IdleWatchGuard />
    </div>
  )
}
