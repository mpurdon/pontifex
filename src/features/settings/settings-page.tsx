import { useMutation, useQuery } from '@tanstack/react-query'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { useEffect, useState } from 'react'
import {
  AlertTriangle,
  Bug,
  Check,
  Database,
  FolderOpen,
  Gauge,
  KeyRound,
  Palette,
  Plus,
  RefreshCw,
  Sparkles,
  Trash2,
  Wrench,
  X,
} from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type {
  BedrockModel,
  ClaudeSettingsImport,
  Environment,
  IpcError,
  ProfileStatus,
  RegistrySummary,
  Settings,
  SsoStatus,
} from '@/lib/types'
import {
  Badge,
  Button,
  Checkbox,
  ErrorBox,
  Field,
  Input,
  Note,
  Panel,
  Segmented,
  Select,
  Spinner,
  StepSlider,
  Textarea,
  cn,
} from '@/components/ui'
import { useSettings } from '@/app/settings-context'
import { useLogin } from '@/app/login-dialog'
import {
  CACHE_BUDGETS_MB,
  SCAN_BUDGETS,
  SCAN_EVENT_CAPS,
  formatCount,
} from '@/lib/format'
import { JiraSection } from '@/features/jira/jira-settings'
import { TextSize, ThemePicker } from '@/theme/theme-picker'
import {
  NO_ROLE,
  hasRole,
  profileRoles,
  ssoBackedEnvironments,
  type CredentialRole,
} from '@/lib/credential-roles'

const TABS = [
  {
    id: 'credentials',
    label: 'Credentials',
    icon: KeyRound,
    hint: 'SSO sessions and AWS profiles',
  },
  {
    id: 'environments',
    label: 'Event bus',
    icon: Database,
    hint: 'Schema registries and log groups per stage',
  },
  {
    id: 'scanning',
    label: 'Scanning',
    icon: Gauge,
    hint: 'How much a scan may read, and how much is cached',
  },
  { id: 'ai', label: 'Bedrock', icon: Sparkles, hint: 'AI schema generation' },
  {
    id: 'jira',
    label: 'Jira',
    icon: Bug,
    hint: 'Filing producer bugs with the team that owns them',
  },
  { id: 'repo', label: 'Repo', icon: FolderOpen, hint: 'Local global-event-bus checkout' },
  { id: 'appearance', label: 'Appearance', icon: Palette, hint: 'Theme and text size' },
  { id: 'developer', label: 'Developer', icon: Wrench, hint: 'Logs and diagnostics' },
] as const

type TabId = (typeof TABS)[number]['id']

export function SettingsPage() {
  const [tab, setTab] = useState<TabId>('credentials')
  const { settings, saveSettings, isSaving } = useSettings()
  // The draft is edited against `base`, the settings it was cloned from, so
  // that anything changed elsewhere while this page is open — the theme
  // picker below, the time-zone toggle, a pane drag — is neither shown as an
  // edit nor put back on save.
  const [edit, setEdit] = useState<{ base: Settings; draft: Settings } | null>(null)
  const [saveError, setSaveError] = useState<IpcError | null>(null)
  const [saved, setSaved] = useState(false)

  useEffect(() => {
    if (settings && !edit) setEdit(fresh(settings))
  }, [settings, edit])

  const profiles = useQuery({
    queryKey: ['profiles'],
    queryFn: ipc.listProfiles,
    retry: false,
  })

  if (!edit || !settings) return <Spinner label="Loading settings…" />
  const { base, draft } = edit

  const patch = (changes: Partial<Settings>) =>
    setEdit((prev) => (prev ? { ...prev, draft: { ...prev.draft, ...changes } } : prev))

  const dirty = JSON.stringify(draft) !== JSON.stringify(base)

  // Save the live settings with only the fields the user edited laid over
  // them. The command still writes wholesale, which keeps the surface small.
  const save = async () => {
    setSaveError(null)
    try {
      const next = await saveSettings({ ...settings, ...changedFields(base, draft) })
      setEdit(fresh(next))
      setSaved(true)
      setTimeout(() => setSaved(false), 2000)
    } catch (e) {
      setSaveError(ipc.asIpcError(e))
    }
  }

  return (
    <div className="flex h-full flex-col">
      <div className="chrome-page flex shrink-0 items-center gap-2 border-b border-edge bg-surface-1 px-3 py-2">
        <h1 className="text-xs font-semibold text-ink">Settings</h1>
        <div className="ml-auto flex items-center gap-2">
          {saved && (
            <Badge tone="ok">
              <Check className="size-2.5" />
              saved
            </Badge>
          )}
          {dirty && (
            <Button variant="ghost" onClick={() => setEdit(fresh(settings))}>
              Discard
            </Button>
          )}
          <Button
            variant="primary"
            disabled={!dirty}
            loading={isSaving}
            onClick={save}
          >
            Save changes
          </Button>
        </div>
      </div>

      <div className="flex min-h-0 flex-1">
        {/*
          Vertical rather than horizontal tabs: the section names are phrases
          ("Event bus environments"), and a horizontal strip runs out of width
          long before the list stops growing.
        */}
        <nav
          className="flex w-40 shrink-0 flex-col gap-0.5 border-r border-edge bg-surface-1 p-2"
          aria-label="Settings sections"
        >
          {TABS.map(({ id, label, icon: Icon, hint }) => (
            <button
              key={id}
              type="button"
              onClick={() => setTab(id)}
              title={hint}
              className={cn(
                'flex items-center gap-2 rounded-md px-2 py-1.5 text-left text-[11px] transition-colors',
                tab === id
                  ? 'bg-accent/15 font-medium text-accent'
                  : 'text-ink-muted hover:bg-surface-2 hover:text-ink',
              )}
            >
              <Icon className="size-3 shrink-0" />
              <span className="truncate">{label}</span>
            </button>
          ))}
        </nav>

        <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto p-3">
          {saveError && <ErrorBox error={saveError} />}

          {tab === 'credentials' && (
            <>
              <SsoSessionsSection />
              <ProfilesSection
                profiles={profiles.data}
                isLoading={profiles.isLoading}
                error={profiles.error ? ipc.asIpcError(profiles.error) : null}
                onRefresh={() => profiles.refetch()}
                settings={draft}
              />
            </>
          )}

          {tab === 'environments' && (
            <EnvironmentsSection
              draft={draft}
              profiles={profiles.data ?? []}
              onChange={patch}
            />
          )}

          {tab === 'scanning' && (
            <>
              <ScanningSection draft={draft} onChange={patch} />
              <EventCacheSection />
            </>
          )}

          {tab === 'ai' && (
            <AiSection draft={draft} profiles={profiles.data ?? []} onChange={patch} />
          )}

          {tab === 'jira' && <JiraSection draft={draft} onChange={patch} />}

          {tab === 'repo' && (
            <>
              <RepoSection draft={draft} onChange={patch} />
              <GithubSection draft={draft} onChange={patch} />
            </>
          )}

          {tab === 'appearance' && (
            <>
              <ThemePicker />
              <TextSize />
            </>
          )}

          {tab === 'developer' && (
            <Panel title="Developer" bodyClassName="flex flex-col gap-2 p-2">
          <Checkbox
            checked={draft.developerMode}
            onChange={(e) => patch({ developerMode: e.target.checked })}
            label="Developer mode"
          />
          <p className="px-1 text-[10px] text-ink-faint">
            Adds a Developer tab with the application log, where Pontifex's files
            live and how large they have grown, and its cache state — for
            checking the app itself is behaving, not the event bus.
          </p>

          <Field
            label="Log level"
            hint="Applies on next launch. Debug records every backend call and its duration; trace adds per-item detail. The Developer tab filters by category, which is what keeps that volume readable."
          >
            <Select
              value={draft.logLevel || 'info'}
              onChange={(e) => patch({ logLevel: e.target.value })}
            >
              <option value="off">off — nothing at all</option>
              <option value="error">error — failures only</option>
              <option value="warn">warn — failures and slow calls</option>
              <option value="info">info — normal operations (default)</option>
              <option value="debug">debug — every backend call</option>
              <option value="trace">trace — everything</option>
            </Select>
          </Field>
            </Panel>
          )}
        </div>
      </div>
    </div>
  )
}

/** A draft to edit, and the snapshot it is measured against. */
function fresh(settings: Settings) {
  return { base: settings, draft: structuredClone(settings) }
}

/** The top-level fields of `draft` that differ from `base`. */
function changedFields(base: Settings, draft: Settings): Partial<Settings> {
  const changed: Partial<Settings> = {}
  for (const key of Object.keys(draft) as (keyof Settings)[]) {
    if (JSON.stringify(draft[key]) !== JSON.stringify(base[key])) {
      Object.assign(changed, { [key]: draft[key] })
    }
  }
  return changed
}

// --- SSO sessions ---------------------------------------------------------

/**
 * Sign in to an SSO session once, then pick accounts and roles from it per
 * environment. This is the path that removes the need for an external
 * credential manager: no profile has to exist for an account to be reachable.
 */
function SsoSessionsSection() {
  const { login } = useLogin()

  const sessions = useQuery({
    queryKey: ['ssoSessions'],
    queryFn: ipc.listSsoSessions,
    retry: false,
  })

  return (
    <Panel
      title="SSO sessions"
      actions={
        <Button
          variant="ghost"
          size="sm"
          onClick={() => sessions.refetch()}
          title="Re-read ~/.aws/config"
        >
          <RefreshCw className="size-3" />
        </Button>
      }
      bodyClassName="p-2"
    >
      <p className="px-1 pb-2 text-[10px] text-ink-faint">
        Sign in once here and every account the session grants becomes available
        to environments below — no AWS profile required, and nothing is written
        to your AWS config.
      </p>

      {sessions.isLoading && <Spinner label="Reading ~/.aws/config…" />}
      {sessions.isError && <ErrorBox error={ipc.asIpcError(sessions.error)} />}

      {sessions.data?.length === 0 && (
        <Note>
          No <span className="font-mono">[sso-session]</span> blocks in
          ~/.aws/config. Add one to use in-app sign-in.
        </Note>
      )}

      {sessions.data && sessions.data.length > 0 && (
        <ul className="flex flex-col gap-1">
          {sessions.data.map((session) => (
            <li
              key={session.name}
              className="flex items-center gap-2 rounded-md border border-edge bg-surface-2 px-2 py-1.5"
            >
              <span className="w-28 shrink-0 truncate font-mono text-[11px] text-ink">
                {session.name}
              </span>
              <span
                className="min-w-0 flex-1 truncate text-[10px] text-ink-faint"
                title={session.startUrl}
              >
                {session.startUrl}
              </span>
              <SsoBadge sso={session.sso} />
              <Button
                variant={session.sso.expired ? 'primary' : 'secondary'}
                size="sm"
                onClick={() => login({ session: session.name })}
              >
                {session.sso.expired ? 'Sign in' : 'Renew'}
              </Button>
            </li>
          ))}
        </ul>
      )}
    </Panel>
  )
}

/**
 * Whether an SSO session currently holds a usable token.
 *
 * Shared by the session list and the profile list — spelled out at both, the
 * expiry tooltip existed in one and not the other.
 */
function SsoBadge({ sso, expiredLabel = 'expired' }: { sso: SsoStatus; expiredLabel?: string }) {
  if (sso.expired) {
    return <Badge tone="warn">{sso.hasToken ? expiredLabel : 'not signed in'}</Badge>
  }
  return (
    <Badge
      tone="ok"
      title={sso.expiresAt ? `Expires ${new Date(sso.expiresAt).toLocaleString()}` : undefined}
    >
      signed in
    </Badge>
  )
}

// --- profiles -------------------------------------------------------------

/**
 * Says what a credential is used for, in the same words everywhere.
 *
 * The two jobs are unrelated and usually live in different accounts, so "which
 * of these is broken, and does it matter for what I am doing" needs to be
 * answerable at a glance.
 */
function RoleBadges({ role }: { role: CredentialRole }) {
  if (!hasRole(role)) {
    return (
      <Badge tone="neutral" title="Not referenced by any environment or by Bedrock">
        unused
      </Badge>
    )
  }
  return (
    <>
      {role.eventBus.length > 0 && (
        <Badge
          tone="accent"
          title={`Reads and writes the schema registry and log groups for: ${role.eventBus.join(', ')}`}
        >
          <Database className="size-2.5" />
          event bus · {role.eventBus.join(', ')}
        </Badge>
      )}
      {role.bedrock && (
        <Badge tone="info" title="Used only for Bedrock model calls (AI schema generation)">
          <Sparkles className="size-2.5" />
          bedrock
        </Badge>
      )}
    </>
  )
}

function ProfilesSection({
  profiles,
  isLoading,
  error,
  onRefresh,
  settings,
}: {
  profiles: ProfileStatus[] | undefined
  isLoading: boolean
  error: IpcError | null
  onRefresh: () => void
  settings: Settings
}) {
  const roles = profileRoles(settings)
  const ssoBacked = ssoBackedEnvironments(settings)

  return (
    <Panel
      title="AWS profiles"
      actions={
        <Button variant="ghost" size="sm" onClick={onRefresh} title="Re-read ~/.aws/config">
          <RefreshCw className="size-3" />
        </Button>
      }
      bodyClassName="p-2"
    >
      <p className="px-1 pb-2 text-[10px] text-ink-faint">
        Every profile in <span className="font-mono">~/.aws/config</span>, tagged
        with what Pontifex uses it for. <span className="text-accent">event bus</span>{' '}
        profiles reach the schema registry and log groups;{' '}
        <span className="text-info">bedrock</span> is used only for AI schema
        generation. A profile with neither tag is not used by Pontifex at all.
      </p>

      {isLoading && <Spinner label="Reading ~/.aws/config…" />}
      {error && <ErrorBox error={error} />}
      {profiles?.length === 0 && (
        <p className="p-2 text-xs text-ink-faint">
          No profiles found in ~/.aws/config.
        </p>
      )}
      {profiles && profiles.length > 0 && (
        <ul className="flex flex-col gap-1">
          {profiles.map((profile) => (
            <ProfileRow
              key={profile.name}
              profile={profile}
              role={roles.get(profile.name) ?? NO_ROLE}
            />
          ))}
        </ul>
      )}

      {ssoBacked.length > 0 && (
        <Note className="mt-2">
          {ssoBacked.length === 1 ? 'One environment does' : `${ssoBacked.length} environments do`}{' '}
          not use a profile at all — {ssoBacked.map((e) => e.label).join(', ')}{' '}
          {ssoBacked.length === 1 ? 'signs' : 'sign'} in through an SSO account
          selected in Environments below, so nothing in this list affects{' '}
          {ssoBacked.length === 1 ? 'it' : 'them'}.
        </Note>
      )}
    </Panel>
  )
}

function ProfileRow({ profile, role }: { profile: ProfileStatus; role: CredentialRole }) {
  const { login } = useLogin()
  const [checking, setChecking] = useState(false)
  const [identity, setIdentity] = useState<string | null>(null)
  const [checkError, setCheckError] = useState<string | null>(null)

  const check = async () => {
    setChecking(true)
    setCheckError(null)
    setIdentity(null)
    try {
      const result = await ipc.checkProfile(profile.name, profile.region ?? undefined)
      if (result.ok) setIdentity(result.identity?.arn ?? 'ok')
      else setCheckError(result.error?.message ?? 'failed')
    } catch (e) {
      setCheckError(ipc.asIpcError(e).message)
    } finally {
      setChecking(false)
    }
  }

  return (
    <li className="flex items-center gap-2 rounded-md border border-edge bg-surface-2 px-2 py-1.5">
      <span
        className={cn(
          'w-56 shrink-0 truncate font-mono text-[11px]',
          hasRole(role) ? 'text-ink' : 'text-ink-faint',
        )}
      >
        {profile.name}
      </span>

      <RoleBadges role={role} />

      <Badge
        tone="neutral"
        title={
          profile.externallyManaged
            ? 'Credentials come from ~/.aws/credentials, written by an external tool such as Leapp'
            : undefined
        }
      >
        {profile.externallyManaged ? 'external' : profile.kind}
      </Badge>

      {profile.sso.applicable && <SsoBadge sso={profile.sso} expiredLabel="token expired" />}

      <span className="truncate text-[10px] text-ink-faint">
        {identity ?? checkError ?? profile.region ?? ''}
      </span>

      <div className="ml-auto flex shrink-0 items-center gap-1">
        <Button variant="ghost" size="sm" loading={checking} onClick={check}>
          Test
        </Button>
        {profile.sso.applicable ? (
          <Button variant="secondary" size="sm" onClick={() => login({ profile: profile.name })}>
            Sign in
          </Button>
        ) : profile.externallyManaged ? (
          <span
            className="text-[10px] text-ink-faint"
            title="Refresh this session in Leapp, aws-vault, or whichever tool wrote it"
          >
            managed externally
          </span>
        ) : null}
      </div>
    </li>
  )
}

// --- environments ---------------------------------------------------------

function EnvironmentsSection({
  draft,
  profiles,
  onChange,
}: {
  draft: Settings
  profiles: ProfileStatus[]
  onChange: (changes: Partial<Settings>) => void
}) {
  const [stages, setStages] = useState<string[]>([])
  useEffect(() => {
    void ipc.listStages().then(setStages)
  }, [])

  const update = (index: number, changes: Partial<Environment>) => {
    const next = draft.environments.map((env, i) =>
      i === index ? { ...env, ...changes } : env,
    )
    onChange({ environments: next })
  }

  const remove = (index: number) => {
    const removed = draft.environments[index]
    const next = draft.environments.filter((_, i) => i !== index)
    onChange({
      environments: next,
      activeEnvironmentId:
        draft.activeEnvironmentId === removed.id
          ? (next[0]?.id ?? null)
          : draft.activeEnvironmentId,
    })
  }

  const addStage = async (stage: string) => {
    const profile = profiles[0]?.name ?? 'default'
    const env = await ipc.defaultEnvironmentForStage(stage, profile)
    // Ids must stay unique; suffix if the stage is already present.
    let id = env.id
    let suffix = 2
    while (draft.environments.some((e) => e.id === id)) {
      id = `${env.id}-${suffix++}`
    }
    onChange({
      environments: [...draft.environments, { ...env, id }],
      activeEnvironmentId: draft.activeEnvironmentId ?? id,
    })
  }

  return (
    <Panel
      title={
        <span className="flex items-center gap-2">
          <Database className="size-3 text-accent" />
          Event bus environments
          <span className="text-[10px] font-normal text-ink-faint">
            schema registry + log groups
          </span>
        </span>
      }
      actions={
        <Select
          value=""
          onChange={(e) => e.target.value && addStage(e.target.value)}
        >
          <option value="">Add stage…</option>
          {stages.map((stage) => (
            <option key={stage} value={stage}>
              {stage}
            </option>
          ))}
        </Select>
      }
      bodyClassName="flex flex-col gap-2 p-2"
    >
      <p className="px-1 text-[10px] text-ink-faint">
        An environment pairs an AWS profile with a registry and its event log
        groups. Defaults follow the SST stack's naming.
      </p>

      {draft.environments.map((env, index) => (
        <div
          key={env.id}
          className={cn(
            'rounded-md border bg-surface-2 p-2',
            env.id === draft.activeEnvironmentId
              ? 'border-accent/50'
              : 'border-edge',
          )}
        >
          <div className="grid grid-cols-[1fr_1fr] gap-2">
            <Field label="Label">
              <Input
                value={env.label}
                onChange={(e) => update(index, { label: e.target.value })}
              />
            </Field>
            <Field label="Region">
              <Input
                value={env.region}
                onChange={(e) => update(index, { region: e.target.value })}
              />
            </Field>
          </div>

          <CredentialField
            env={env}
            profiles={profiles}
            onChange={(changes) => update(index, changes)}
          />

          <div className="mt-2 grid grid-cols-[1fr_1fr] gap-2">
            <RegistryField
              env={env}
              onChange={(registryName) => update(index, { registryName })}
            />
            <Field
              label="Log groups"
              hint="comma separated"
            >
              <Input
                value={env.logGroups.join(', ')}
                onChange={(e) =>
                  update(index, {
                    logGroups: e.target.value
                      .split(',')
                      .map((s) => s.trim())
                      .filter(Boolean),
                  })
                }
                className="font-mono"
              />
            </Field>
          </div>

          <div className="mt-2 flex items-center gap-3">
            <Checkbox
              checked={env.protected}
              onChange={(e) => update(index, { protected: e.target.checked })}
              label="Protected (extra confirmation on deletes)"
            />
            <Button
              variant="ghost"
              size="sm"
              className="ml-auto text-danger"
              onClick={() => remove(index)}
            >
              <Trash2 className="size-3" />
              Remove
            </Button>
          </div>
        </div>
      ))}
    </Panel>
  )
}

/**
 * Where an environment gets its credentials: an SSO account+role resolved
 * in-app, or a named AWS profile.
 *
 * The SSO path is the one that removes the need for an external credential
 * manager — sign in to the session once and any account it grants becomes
 * reachable, with nothing written to ~/.aws.
 */
function CredentialField({
  env,
  profiles,
  onChange,
}: {
  env: Environment
  profiles: ProfileStatus[]
  onChange: (changes: Partial<Environment>) => void
}) {
  const usingSso = !!env.sso
  const [showAllAccounts, setShowAllAccounts] = useState(false)

  const sessions = useQuery({
    queryKey: ['ssoSessions'],
    queryFn: ipc.listSsoSessions,
    retry: false,
  })

  const session =
    env.sso?.session ?? sessions.data?.find((s) => !s.sso.expired)?.name ?? sessions.data?.[0]?.name

  const signedIn = sessions.data?.find((s) => s.name === session && !s.sso.expired)

  const accounts = useQuery({
    queryKey: ['ssoAccounts', session],
    queryFn: () => ipc.listSsoAccounts(session!),
    // Only meaningful once the session has a live token.
    enabled: usingSso && !!session && !!signedIn,
    staleTime: 5 * 60 * 1000,
    retry: false,
  })

  const roles = useQuery({
    queryKey: ['ssoRoles', session, env.sso?.accountId],
    queryFn: () => ipc.listSsoAccountRoles(session!, env.sso!.accountId),
    enabled: usingSso && !!session && !!signedIn && !!env.sso?.accountId,
    staleTime: 5 * 60 * 1000,
    retry: false,
  })

  const visibleAccounts = (accounts.data ?? []).filter(
    (a) => showAllAccounts || a.matchesEventBus,
  )
  const eventBusCount = (accounts.data ?? []).filter((a) => a.matchesEventBus).length

  const switchTo = (mode: 'sso' | 'profile') => {
    if (mode === 'profile') {
      onChange({ sso: null })
    } else {
      onChange({
        sso: {
          session: session ?? '',
          accountId: env.sso?.accountId ?? '',
          roleName: env.sso?.roleName ?? '',
        },
      })
    }
  }

  return (
    <div className="mt-2 rounded-md border border-edge/60 bg-surface-1 p-2">
      <div className="mb-2 flex items-center gap-2">
        <span className="text-[11px] font-medium text-ink-muted">Credentials</span>
        <Segmented
          value={usingSso ? 'sso' : 'profile'}
          options={[
            { id: 'sso', label: 'SSO account' },
            { id: 'profile', label: 'AWS profile' },
          ]}
          onChange={switchTo}
        />
        {usingSso && <ExportProfileButton env={env} />}
      </div>

      {usingSso ? (
        <div className="flex flex-col gap-2">
          {sessions.data && sessions.data.length === 0 && (
            <Note>
              No <span className="font-mono">[sso-session]</span> in
              ~/.aws/config to sign into.
            </Note>
          )}

          <div className="grid grid-cols-[140px_1fr_180px] gap-2">
            <Field label="Session">
              <Select
                value={env.sso?.session ?? ''}
                onChange={(e) =>
                  onChange({
                    sso: {
                      session: e.target.value,
                      accountId: '',
                      roleName: '',
                    },
                  })
                }
                className="w-full"
              >
                <option value="">choose…</option>
                {sessions.data?.map((s) => (
                  <option key={s.name} value={s.name}>
                    {s.name}
                  </option>
                ))}
              </Select>
            </Field>

            <Field
              label="Account"
              hint={
                accounts.isError ? (
                  <span className="text-danger">
                    {ipc.asIpcError(accounts.error).message}
                  </span>
                ) : accounts.data && !showAllAccounts ? (
                  `${eventBusCount} Global Event Bus of ${accounts.data.length}`
                ) : undefined
              }
            >
              <div className="flex gap-1">
                <Select
                  value={env.sso?.accountId ?? ''}
                  disabled={!signedIn || accounts.isLoading}
                  onChange={(e) => {
                    const picked = accounts.data?.find(
                      (a) => a.accountId === e.target.value,
                    )
                    onChange({
                      sso: {
                        session: env.sso?.session ?? session ?? '',
                        accountId: e.target.value,
                        roleName: '',
                        accountName: picked?.accountName ?? undefined,
                      },
                    })
                  }}
                  className="w-full"
                >
                  <option value="">
                    {!signedIn
                      ? 'sign in to the session first'
                      : accounts.isLoading
                        ? 'loading…'
                        : 'choose…'}
                  </option>
                  {/* Keep a previously chosen account selectable even when the
                      filter would hide it. */}
                  {env.sso?.accountId &&
                    !visibleAccounts.some((a) => a.accountId === env.sso!.accountId) && (
                      <option value={env.sso.accountId}>
                        {env.sso.accountName ?? env.sso.accountId}
                      </option>
                    )}
                  {visibleAccounts.map((account) => (
                    <option key={account.accountId} value={account.accountId}>
                      {account.accountName ?? account.accountId} · {account.accountId}
                    </option>
                  ))}
                </Select>
                <Button
                  variant="ghost"
                  size="md"
                  onClick={() => setShowAllAccounts((v) => !v)}
                  title={
                    showAllAccounts
                      ? 'Show only Global Event Bus accounts'
                      : 'Show all accounts'
                  }
                >
                  {showAllAccounts ? 'GEB' : 'All'}
                </Button>
              </div>
            </Field>

            <Field
              label="Role"
              hint={
                roles.isError ? (
                  <span className="text-danger">
                    {ipc.asIpcError(roles.error).message}
                  </span>
                ) : undefined
              }
            >
              <Select
                value={env.sso?.roleName ?? ''}
                disabled={!env.sso?.accountId || roles.isLoading}
                onChange={(e) =>
                  onChange({
                    sso: { ...env.sso!, roleName: e.target.value },
                  })
                }
                className="w-full"
              >
                <option value="">
                  {roles.isLoading ? 'loading…' : 'choose…'}
                </option>
                {env.sso?.roleName &&
                  !(roles.data ?? []).includes(env.sso.roleName) && (
                    <option value={env.sso.roleName}>{env.sso.roleName}</option>
                  )}
                {roles.data?.map((role) => (
                  <option key={role} value={role}>
                    {role}
                  </option>
                ))}
              </Select>
            </Field>
          </div>
        </div>
      ) : (
        <div className="flex flex-col gap-2">
          <Field label="AWS profile">
            <Select
              value={env.awsProfile}
              onChange={(e) => onChange({ awsProfile: e.target.value })}
              className="w-full"
            >
              <option value="">choose…</option>
              {env.awsProfile && !profiles.some((p) => p.name === env.awsProfile) && (
                <option value={env.awsProfile}>{env.awsProfile} (missing)</option>
              )}
              {profiles.map((profile) => (
                <option key={profile.name} value={profile.name}>
                  {profile.name}
                  {profile.externallyManaged ? ' (external)' : ''}
                </option>
              ))}
            </Select>
          </Field>

          {/* A vanished profile is the common failure with externally managed
              credentials, and the fix is usually to switch to in-app SSO. */}
          {env.awsProfile &&
            profiles.length > 0 &&
            !profiles.some((p) => p.name === env.awsProfile) && (
              <div className="flex items-start gap-2 rounded-md border border-warn/40 bg-warn/10 px-2 py-1.5 text-[10px] text-warn">
                <AlertTriangle className="mt-px size-3 shrink-0" />
                <span className="min-w-0 flex-1">
                  <span className="font-mono">{env.awsProfile}</span> is not in
                  ~/.aws/config. Tools that write temporary credentials remove
                  the profile when the session ends.{' '}
                  <button
                    type="button"
                    className="underline"
                    onClick={() => switchTo('sso')}
                  >
                    Use an SSO account instead
                  </button>{' '}
                  to avoid this.
                </span>
              </div>
            )}
        </div>
      )}
    </div>
  )
}

/** Opt-in export of an SSO target to ~/.aws/config, for CLI use. */
function ExportProfileButton({ env }: { env: Environment }) {
  const [name, setName] = useState(`gm-${env.id}`)
  const [open, setOpen] = useState(false)

  const exportProfile = useMutation<string, IpcError>({
    mutationFn: () => ipc.exportSsoProfile(env.id, name),
  })

  if (!open) {
    return (
      <Button
        variant="ghost"
        size="sm"
        className="ml-auto"
        disabled={!env.sso?.accountId || !env.sso?.roleName}
        onClick={() => setOpen(true)}
        title="Write this account+role to ~/.aws/config so the AWS CLI can use it too"
      >
        Export as AWS profile
      </Button>
    )
  }

  return (
    <div className="ml-auto flex items-center gap-1">
      <Input
        value={name}
        onChange={(e) => setName(e.target.value)}
        className="w-36 font-mono"
        spellCheck={false}
      />
      <Button
        variant="primary"
        size="sm"
        loading={exportProfile.isPending}
        onClick={() => exportProfile.mutate()}
      >
        Write
      </Button>
      <Button
        variant="ghost"
        size="sm"
        onClick={() => {
          setOpen(false)
          exportProfile.reset()
        }}
      >
        <X className="size-3" />
      </Button>
      {exportProfile.isSuccess && (
        <Badge tone="ok" title={exportProfile.data}>
          written
        </Badge>
      )}
      {exportProfile.isError && (
        <Badge tone="danger" title={exportProfile.error.message}>
          failed
        </Badge>
      )}
    </div>
  )
}

/**
 * Registry name with discovery.
 *
 * `<stage>-global-registry` holds for the long-lived stages, but sandbox
 * deploys create one registry per pull request (`PR-213-global-registry`), so
 * typing the name from memory does not work there. Discovery lists what the
 * environment's credentials can actually see.
 */
function RegistryField({
  env,
  onChange,
}: {
  env: Environment
  onChange: (registryName: string) => void
}) {
  const [found, setFound] = useState<RegistrySummary[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)

  const discover = async () => {
    setLoading(true)
    setError(null)
    try {
      setFound(await ipc.listRegistries(env.id))
    } catch (e) {
      setError(ipc.asIpcError(e).message)
    } finally {
      setLoading(false)
    }
  }

  return (
    <Field
      label="Schema registry"
      hint={
        error ? (
          <span className="text-danger">{error}</span>
        ) : found ? (
          `${found.length} visible in this account`
        ) : undefined
      }
    >
      <div className="flex gap-1">
        {found && found.length > 0 ? (
          <Select
            value={found.some((r) => r.name === env.registryName) ? env.registryName : ''}
            onChange={(e) => onChange(e.target.value)}
            className="w-full font-mono"
          >
            {!found.some((r) => r.name === env.registryName) && (
              <option value="">{env.registryName} (not found)</option>
            )}
            {found.map((registry) => (
              <option key={registry.name} value={registry.name}>
                {registry.name}
                {registry.tags['global-event-bus']
                  ? ` — ${registry.tags['global-event-bus']}`
                  : ''}
              </option>
            ))}
          </Select>
        ) : (
          <Input
            value={env.registryName}
            onChange={(e) => onChange(e.target.value)}
            className="font-mono"
            spellCheck={false}
          />
        )}
        <Button
          variant="secondary"
          size="md"
          loading={loading}
          onClick={discover}
          title="List registries this profile can see"
        >
          {found ? <RefreshCw className="size-3" /> : 'Discover'}
        </Button>
      </div>
    </Field>
  )
}

// --- AI -------------------------------------------------------------------

function AiSection({
  draft,
  profiles,
  onChange,
}: {
  draft: Settings
  profiles: ProfileStatus[]
  onChange: (changes: Partial<Settings>) => void
}) {
  const [importResult, setImportResult] = useState<ClaudeSettingsImport | null>(null)

  const doImport = useMutation<ClaudeSettingsImport, IpcError>({
    mutationFn: () => ipc.importClaudeSettings(draft.llm.claudeSettingsPath ?? undefined),
    onSuccess: (result) => {
      setImportResult(result)
      onChange({
        llm: {
          ...draft.llm,
          models: result.models,
          selectedModelId: result.models[0]?.id ?? null,
          awsProfile: draft.llm.awsProfile ?? result.awsProfile,
          region: result.region ?? draft.llm.region,
          claudeSettingsPath: result.sourcePath,
        },
      })
    },
  })

  const discover = useMutation<BedrockModel[], IpcError>({
    mutationFn: ipc.listBedrockModels,
    onSuccess: (models) => {
      onChange({
        llm: {
          ...draft.llm,
          models,
          selectedModelId: draft.llm.selectedModelId ?? models[0]?.id ?? null,
        },
      })
    },
  })

  const setLlm = (changes: Partial<Settings['llm']>) =>
    onChange({ llm: { ...draft.llm, ...changes } })

  return (
    <Panel
      title={
        <span className="flex items-center gap-2">
          <Sparkles className="size-3 text-info" />
          Bedrock
          <span className="text-[10px] font-normal text-ink-faint">
            AI schema generation
          </span>
        </span>
      }
      bodyClassName="flex flex-col gap-3 p-2"
    >
      <p className="px-1 text-[10px] text-ink-faint">
        This account is used <em>only</em> to call Bedrock models. It never
        touches the schema registry or the log groups, so it can be — and
        usually is — a different account from the event bus environments above.
      </p>

      <div className="grid grid-cols-2 gap-2">
        <Field label="AWS profile for Bedrock">
          <Select
            value={draft.llm.awsProfile ?? ''}
            onChange={(e) => setLlm({ awsProfile: e.target.value || null })}
            className="w-full"
          >
            <option value="">none</option>
            {profiles.map((profile) => (
              <option key={profile.name} value={profile.name}>
                {profile.name}
              </option>
            ))}
          </Select>
        </Field>
        <Field label="Bedrock region">
          <Input
            value={draft.llm.region}
            onChange={(e) => setLlm({ region: e.target.value })}
          />
        </Field>
      </div>

      <Field
        label="Claude Code settings file"
        hint="Where the ANTHROPIC_DEFAULT_*_MODEL inference profile ARNs are read from. Leave blank to search ~/.claude automatically."
      >
        <div className="flex gap-2">
          <Input
            value={draft.llm.claudeSettingsPath ?? ''}
            onChange={(e) => setLlm({ claudeSettingsPath: e.target.value || null })}
            placeholder="~/.claude/trajector-settings.json"
            className="font-mono"
            spellCheck={false}
          />
          <Button
            variant="secondary"
            loading={doImport.isPending}
            onClick={() => doImport.mutate()}
          >
            Import
          </Button>
          <Button
            variant="ghost"
            loading={discover.isPending}
            onClick={() => discover.mutate()}
            title="List inference profiles from Bedrock instead"
          >
            Discover
          </Button>
        </div>
      </Field>

      {doImport.isError && <ErrorBox error={doImport.error} />}
      {discover.isError && <ErrorBox error={discover.error} />}

      {importResult && (
        <Note>
          Imported {importResult.models.length} model
          {importResult.models.length === 1 ? '' : 's'} from{' '}
          <span className="font-mono">{importResult.sourcePath}</span>
        </Note>
      )}

      {draft.llm.models.length > 0 && (
        <div className="flex flex-col gap-1">
          <span className="px-1 text-[11px] font-medium text-ink-muted">
            Models
          </span>
          <ul className="flex flex-col gap-1">
            {draft.llm.models.map((model) => (
              <li
                key={model.id}
                className="flex items-center gap-2 rounded-md border border-edge bg-surface-2 px-2 py-1"
              >
                <input
                  type="radio"
                  name="selectedModel"
                  checked={draft.llm.selectedModelId === model.id}
                  onChange={() => setLlm({ selectedModelId: model.id })}
                  className="size-3 accent-accent"
                />
                <span className="w-24 shrink-0 text-[11px] text-ink">
                  {model.label}
                </span>
                <span
                  className="truncate font-mono text-[10px] text-ink-faint"
                  title={model.modelId}
                >
                  {model.modelId}
                </span>
                <Button
                  variant="ghost"
                  size="sm"
                  className="ml-auto"
                  onClick={() =>
                    setLlm({
                      models: draft.llm.models.filter((m) => m.id !== model.id),
                    })
                  }
                >
                  <X className="size-3" />
                </Button>
              </li>
            ))}
          </ul>
          <Button
            variant="ghost"
            size="sm"
            className="self-start"
            onClick={() =>
              setLlm({
                models: [
                  ...draft.llm.models,
                  { id: `custom-${draft.llm.models.length}`, label: 'Custom', modelId: '' },
                ],
              })
            }
          >
            <Plus className="size-3" />
            Add model manually
          </Button>
        </div>
      )}
    </Panel>
  )
}

// --- scanning -------------------------------------------------------------

/**
 * How much work a scan may do.
 *
 * These three were scattered and mostly invisible: the time budget was a
 * toolbar slider, the event cap a literal in the frontend, and the cache size
 * a Rust constant. They are one subject — what sampling costs you in time,
 * API spend and disk — so they belong together.
 */
function ScanningSection({
  draft,
  onChange,
}: {
  draft: Settings
  onChange: (changes: Partial<Settings>) => void
}) {
  const setScan = (changes: Partial<Settings['scan']>) =>
    onChange({ scan: { ...draft.scan, ...changes } })

  const perSlice = Math.ceil(draft.scan.maxEvents / 8)

  return (
    <Panel
      title={
        <span className="flex items-center gap-2">
          <Gauge className="size-3 text-accent" />
          Scanning
          <span className="text-[10px] font-normal text-ink-faint">
            CloudWatch sampling limits
          </span>
        </span>
      }
      bodyClassName="flex flex-col gap-3 p-3"
    >
      <p className="text-[10px] text-ink-faint">
        A scan splits its time window into 8 slices and reads them in parallel,
        so the sample is spread across the whole period rather than bunched at
        the recent end. These bound what that costs.
      </p>

      <Field
        label="Time budget"
        hint="How long a scan may run before it stops. It bounds how long you wait, not how much of the window is covered — a smaller budget thins every slice evenly rather than truncating one end."
      >
        <StepSlider
          value={draft.scan.maxSeconds}
          steps={SCAN_BUDGETS}
          onChange={(maxSeconds) => setScan({ maxSeconds })}
          format={(v) => `${v}s`}
        />
      </Field>

      <Field
        label="Event cap"
        hint={`Events collected per scan, divided across the slices — about ${formatCount(perSlice)} each. Raise it when the report says slices hit their cap; that means those periods were busier than their share.`}
      >
        <StepSlider
          value={draft.scan.maxEvents}
          steps={SCAN_EVENT_CAPS}
          onChange={(maxEvents) => setScan({ maxEvents })}
          format={formatCount}
        />
      </Field>

      <Field
        label="Cache budget"
        hint="Disk the sampled-event cache may use. Past this, the least-recently-fetched event types are evicted. Payload sizes vary hugely — some carry kilobyte-long presigned URLs — so a count cap alone does not bound the footprint."
      >
        <StepSlider
          value={draft.scan.cacheMb}
          steps={CACHE_BUDGETS_MB}
          onChange={(cacheMb) => setScan({ cacheMb })}
          format={(v) => `${v} MB`}
        />
      </Field>
    </Panel>
  )
}

// --- event cache ----------------------------------------------------------

/**
 * Sampled events kept on disk so reality checks and the health report do not
 * re-scan CloudWatch. Exposed here because it is real disk usage and because a
 * stale sample is occasionally worth discarding.
 */
function EventCacheSection() {
  const { envId, activeEnvironment } = useSettings()

  const stats = useQuery({
    queryKey: ['eventCacheStats'],
    queryFn: ipc.eventCacheStats,
    retry: false,
  })

  const clear = useMutation<number, IpcError, { all: boolean }>({
    mutationFn: ({ all }) => ipc.clearEventCache(envId, all),
    onSuccess: () => stats.refetch(),
  })

  const [types, events] = stats.data ?? [0, 0]

  return (
    <Panel
      title="Cached events"
      actions={
        <Button variant="ghost" size="sm" onClick={() => stats.refetch()}>
          <RefreshCw className="size-3" />
        </Button>
      }
      bodyClassName="flex flex-col gap-2 p-2"
    >
      <p className="px-1 text-[10px] text-ink-faint">
        Event samples are cached on disk so a reality check can re-run as you
        edit without another CloudWatch scan. Running the health report warms
        this for every event type it sees.
      </p>

      <div className="flex items-center gap-2 px-1 text-xs text-ink-muted">
        <Badge tone="neutral">{types} event types</Badge>
        <Badge tone="neutral">{events.toLocaleString()} events</Badge>
      </div>

      {clear.isError && <ErrorBox error={clear.error} />}
      {clear.isSuccess && (
        <Note>Cleared {clear.data} cached event types.</Note>
      )}

      <div className="flex gap-2">
        <Button
          variant="secondary"
          size="sm"
          loading={clear.isPending}
          onClick={() => clear.mutate({ all: false })}
          disabled={types === 0}
        >
          Clear {activeEnvironment?.label ?? 'this environment'}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          loading={clear.isPending}
          onClick={() => clear.mutate({ all: true })}
          disabled={types === 0}
        >
          Clear all
        </Button>
      </div>
    </Panel>
  )
}

// --- repo -----------------------------------------------------------------

/**
 * GitHub access for the origin lookup: which organisation to search and a
 * token to search it with. The token lives in the keychain, and someone
 * already signed into the GitHub CLI needs no token here at all.
 */
function GithubSection({
  draft,
  onChange,
}: {
  draft: Settings
  onChange: (changes: Partial<Settings>) => void
}) {
  const status = useQuery({
    queryKey: ['github', 'status'],
    queryFn: ipc.githubStatus,
    retry: false,
  })
  const [token, setToken] = useState('')
  const save = useMutation<void, IpcError, string>({
    mutationFn: ipc.setGithubToken,
    onSuccess: () => {
      setToken('')
      status.refetch()
    },
  })
  const check = useMutation<string, IpcError>({ mutationFn: ipc.githubCheck })
  const tokenSource = status.data?.tokenSource
  return (
    <Panel title="GitHub" bodyClassName="flex flex-col gap-3 p-3">
      <p className="text-[10px] text-ink-faint">
        The Origin view on a schema, and on a watch hit, searches the organisation's code for
        the event type to say who first published it, in which pull request, and who owns that
        code now.
      </p>
      <Field
        label="Organisation"
        hint={
          status.data?.orgFromRemote
            ? `Left blank, ${status.data.orgFromRemote} is read from the bus checkout's remote.`
            : 'The GitHub organisation whose repositories publish events.'
        }
      >
        <Input
          value={draft.github.org ?? ''}
          onChange={(e) => onChange({ github: { ...draft.github, org: e.target.value || null } })}
          placeholder={status.data?.orgFromRemote ?? 'team-and-tech'}
          className="font-mono"
          spellCheck={false}
        />
      </Field>
      <Field
        label="Token"
        hint={
          tokenSource === 'ghCli'
            ? 'Borrowing the GitHub CLI sign-in. Store a token here only to override it.'
            : tokenSource === 'keychain'
              ? 'A token is stored in the keychain — leave blank to keep it, or save an empty value to clear it.'
              : 'A personal access token with read access to the organisation, or sign in with `gh auth login` and leave this blank.'
        }
      >
        <div className="flex gap-2">
          <Input
            type="password"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            placeholder={tokenSource === 'keychain' ? '••••••••' : 'ghp_…'}
            className="font-mono"
            spellCheck={false}
          />
          <Button
            variant="secondary"
            onClick={() => save.mutate(token)}
            loading={save.isPending}
            disabled={!token && tokenSource !== 'keychain'}
          >
            {token ? 'Save token' : 'Clear'}
          </Button>
          <Button variant="ghost" onClick={() => check.mutate()} loading={check.isPending}>
            Check
          </Button>
        </div>
      </Field>
      {check.data && <Note tone="ok">Signed in as {check.data}.</Note>}
      {check.isError && <ErrorBox error={check.error} />}
      {save.isError && <ErrorBox error={save.error} />}

      <Field
        label="Never examine"
        hint="One path glob per line, with CODEOWNERS rules: * within a name, ** across directories, a leading / anchors to the repo root. Matching files are skipped before any history is read, so everything left is examined in the first pass."
      >
        <Textarea
          rows={6}
          value={draft.github.ignore.join('\n')}
          onChange={(e) =>
            onChange({
              github: {
                ...draft.github,
                ignore: e.target.value
                  .split('\n')
                  .map((l) => l.trim())
                  .filter((l) => l && !l.startsWith('#')),
              },
            })
          }
          placeholder={(status.data?.defaultIgnore ?? []).join('\n')}
          className="font-mono"
          spellCheck={false}
        />
      </Field>
      <div>
        <Button
          variant="ghost"
          size="sm"
          disabled={!status.data}
          onClick={() =>
            onChange({ github: { ...draft.github, ignore: [...(status.data?.defaultIgnore ?? [])] } })
          }
        >
          Reset to defaults
        </Button>
      </div>
    </Panel>
  )
}

function RepoSection({
  draft,
  onChange,
}: {
  draft: Settings
  onChange: (changes: Partial<Settings>) => void
}) {
  const [preview, setPreview] = useState<string | null>(null)
  const [previewError, setPreviewError] = useState<string | null>(null)

  const browse = async () => {
    const picked = await openDialog({
      directory: true,
      multiple: false,
      title: 'Choose the global-event-bus checkout',
    })
    if (typeof picked === 'string') onChange({ eventBusRepoPath: picked })
  }

  const check = async () => {
    setPreview(null)
    setPreviewError(null)
    try {
      const buses = await ipc.previewBusConfiguration(draft.eventBusRepoPath ?? '')
      setPreview(`Found ${buses.length} declared buses across all stages.`)
    } catch (e) {
      setPreviewError(ipc.asIpcError(e).message)
    }
  }

  return (
    <Panel title="global-event-bus repo" bodyClassName="flex flex-col gap-2 p-2">
      <p className="px-1 text-[10px] text-ink-faint">
        Used by the topology view to read{' '}
        <span className="font-mono">stacks/busConfiguration.ts</span>, and as the
        default directory for import and export.
      </p>

      <div className="flex gap-2">
        <Input
          value={draft.eventBusRepoPath ?? ''}
          onChange={(e) => onChange({ eventBusRepoPath: e.target.value || null })}
          placeholder="~/Projects/trajector/global-event-bus"
          className="font-mono"
          spellCheck={false}
        />
        <Button variant="secondary" onClick={browse}>
          <FolderOpen className="size-3" />
          Browse
        </Button>
        <Button
          variant="ghost"
          onClick={check}
          disabled={!draft.eventBusRepoPath}
        >
          Check
        </Button>
      </div>

      {preview && <Note>{preview}</Note>}
      {previewError && (
        <ErrorBox error={{ kind: 'notFound', message: previewError }} />
      )}
    </Panel>
  )
}
