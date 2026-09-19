import { useMemo, useState } from 'react'
import {
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
} from '@tanstack/react-query'
import {
  Bug,
  CheckCircle2,
  ExternalLink,
  Plus,
  Trash2,
  Unplug,
} from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type {
  AuthorizeUrlParts,
  IpcError,
  JiraProject,
  Settings,
  SourceRoute,
} from '@/lib/types'
import {
  Badge,
  Button,
  ErrorBox,
  Field,
  Input,
  Note,
  Panel,
  Select,
  Spinner,
  cn,
} from '@/components/ui'
import { useSettings } from '@/app/settings-context'

/**
 * Wildcard families worth offering, derived from the sources that exist.
 *
 * A source is conventionally `team-service`, so the leading segment names the
 * family. Only suggested where more than one source shares it — `billing-*` is
 * a useful rule, `billing-invoices-*` matching one thing is just a slower way
 * of writing its name.
 */
export function suggestPatterns(sources: string[]): string[] {
  const families = new Map<string, number>()
  for (const source of sources) {
    const [head] = source.split(/[-. ]/)
    if (head && head !== source) {
      families.set(head, (families.get(head) ?? 0) + 1)
    }
  }
  const wildcards = [...families.entries()]
    .filter(([, count]) => count > 1)
    .map(([head]) => `${head}-*`)
    .sort()
  return [...wildcards, ...sources]
}

/**
 * A project chosen from Jira, or typed when Jira cannot be reached.
 *
 * Both the per-rule and the default-project fields need the same three
 * behaviours: pick from the real list, keep showing a key that is set but not
 * visible, and degrade to free text when the list could not be read.
 */
function ProjectPicker({
  value,
  projects,
  emptyLabel,
  onChange,
}: {
  value: string
  /** Undefined until Jira answers — or forever, if it cannot. */
  projects: JiraProject[] | undefined
  emptyLabel: string
  onChange: (key: string) => void
}) {
  if (!projects) {
    return (
      <Input
        value={value}
        onChange={(e) => onChange(e.target.value.toUpperCase())}
        placeholder="IPP"
        className="font-mono"
        spellCheck={false}
      />
    )
  }

  return (
    <Select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="w-full font-mono"
    >
      <option value="">{emptyLabel}</option>
      {/* A key set before the list loaded, or from a project you can no longer
          see, still shows rather than silently vanishing from the form. */}
      {value && !projects.some((p) => p.key === value) && (
        <option value={value}>{value} — not visible to you</option>
      )}
      {projects.map((project) => (
        <option key={project.key} value={project.key}>
          {project.key} — {project.name}
        </option>
      ))}
    </Select>
  )
}

/** An issue type this project offers, or free text until it says. */
function IssueTypePicker({
  value,
  types,
  fallbackLabel,
  onChange,
}: {
  value: string
  types: string[] | undefined
  /** Shown for the empty choice, or for a value the project does not offer. */
  fallbackLabel: string
  onChange: (type: string) => void
}) {
  if (!types) {
    return (
      <Input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={fallbackLabel}
        spellCheck={false}
      />
    )
  }

  return (
    <Select value={value} onChange={(e) => onChange(e.target.value)} className="w-full">
      {!types.includes(value) && <option value={value}>{value || fallbackLabel}</option>}
      {types.map((type) => (
        <option key={type} value={type}>
          {type}
        </option>
      ))}
    </Select>
  )
}

/** One column layout for the header and every rule row, so they cannot drift. */
const ROUTE_GRID = 'grid grid-cols-[1fr_190px_150px_32px] gap-2'

/**
 * Connecting to Jira, and deciding which team hears about which producer.
 *
 * Two halves that look alike but are not: the app registration is a one-time
 * act by whoever has an Atlassian admin's ear, and the routing table is the
 * part that gets edited as teams and services move around.
 */
const blankRoute = (): SourceRoute => ({
  id: crypto.randomUUID(),
  pattern: '',
  projectKey: '',
  issueType: null,
  labels: [],
  assigneeAccountId: null,
})

/**
 * The rows of one routing table: a pattern, a project, an issue type.
 *
 * Source rules and owner rules have the same shape and differ only in what
 * the pattern is held against, so they share the editor and say so in the
 * header.
 */
function RouteRows({
  routes,
  emptyText,
  patternHeader,
  patternPlaceholder,
  patternListId,
  projects,
  issueTypesByProject,
  defaultIssueType,
  onChange,
}: {
  routes: SourceRoute[]
  emptyText: string
  patternHeader: string
  patternPlaceholder: string
  patternListId?: string
  projects: JiraProject[] | undefined
  issueTypesByProject: Map<string, string[]>
  defaultIssueType: string
  onChange: (routes: SourceRoute[]) => void
}) {
  const patch = (id: string, changes: Partial<SourceRoute>) =>
    onChange(routes.map((route) => (route.id === id ? { ...route, ...changes } : route)))

  if (routes.length === 0) {
    return <p className="px-1 py-2 text-[11px] text-ink-faint">{emptyText}</p>
  }
  return (
    <div className="flex flex-col gap-1">
      {/* Labels once, above the columns. Repeating a labelled Field per row
          let a hint under one control push its neighbours out of line, so no
          two rows sat at the same height. */}
      <div className={cn(ROUTE_GRID, 'px-1 text-[10px] text-ink-faint')}>
        <span>{patternHeader}</span>
        <span>Project</span>
        <span>Issue type</span>
        <span />
      </div>

      {routes.map((route) => (
        <div key={route.id} className={cn(ROUTE_GRID, 'items-center')}>
          <Input
            value={route.pattern}
            onChange={(e) => patch(route.id, { pattern: e.target.value })}
            placeholder={patternPlaceholder}
            className="font-mono"
            spellCheck={false}
            list={patternListId}
          />

          <ProjectPicker
            value={route.projectKey}
            projects={projects}
            emptyLabel="choose…"
            onChange={(key) => patch(route.id, { projectKey: key })}
          />

          <IssueTypePicker
            value={route.issueType ?? ''}
            types={issueTypesByProject.get(route.projectKey)}
            fallbackLabel={`${defaultIssueType} (default)`}
            onChange={(type) => patch(route.id, { issueType: type || null })}
          />

          <Button
            variant="ghost"
            size="sm"
            className="text-danger"
            onClick={() => onChange(routes.filter((r) => r.id !== route.id))}
            title="Remove this rule"
          >
            <Trash2 className="size-3" />
          </Button>
        </div>
      ))}
    </div>
  )
}

export function JiraSection({
  draft,
  onChange,
}: {
  draft: Settings
  onChange: (changes: Partial<Settings>) => void
}) {
  const queryClient = useQueryClient()
  const patchJira = (changes: Partial<Settings['jira']>) =>
    onChange({ jira: { ...draft.jira, ...changes } })

  const [secret, setSecret] = useState('')
  const [clientId, setClientId] = useState(draft.jira.clientId)
  /** What the console's URL said, so the panel can report on it. */
  const [pasted, setPasted] = useState<AuthorizeUrlParts | null>(null)
  const [pasteError, setPasteError] = useState<IpcError | null>(null)

  /**
   * Fill the fields from the authorization URL the console generates.
   *
   * Everything pontifex needs is already in that URL — client id, the callback
   * as registered, and the scopes actually granted. Retyping them is how a
   * single wrong character becomes a consent-screen error that mentions none
   * of this.
   */
  const readUrl = async (url: string) => {
    if (!url.trim()) {
      setPasted(null)
      setPasteError(null)
      return
    }
    try {
      const parts = await ipc.parseJiraAuthorizeUrl(url)
      setPasted(parts)
      setPasteError(null)
      setClientId(parts.clientId)
      patchJira({ callbackUrl: parts.callbackUrl })
    } catch (e) {
      setPasted(null)
      setPasteError(ipc.asIpcError(e))
    }
  }

  const status = useQuery({
    queryKey: ['jira', 'status'],
    queryFn: ipc.jiraStatus,
    retry: false,
  })

  const invalidate = () => queryClient.invalidateQueries({ queryKey: ['jira'] })

  const saveApp = useMutation<unknown, IpcError>({
    mutationFn: () =>
      ipc.setJiraApp(clientId, secret || undefined, draft.jira.callbackUrl),
    onSuccess: () => {
      // Never held in component state longer than the request: the whole point
      // of the keychain is that this value does not sit in memory or on disk.
      setSecret('')
      invalidate()
    },
  })

  const connect = useMutation<unknown, IpcError>({
    mutationFn: ipc.jiraConnect,
    onSuccess: invalidate,
  })

  const disconnect = useMutation<unknown, IpcError>({
    mutationFn: ipc.jiraDisconnect,
    onSuccess: invalidate,
  })

  const sites = useQuery({
    queryKey: ['jira', 'sites'],
    queryFn: ipc.jiraSites,
    enabled: !!status.data?.connected,
    retry: false,
  })

  const selectSite = useMutation<unknown, IpcError, string>({
    mutationFn: ipc.selectJiraSite,
    onSuccess: invalidate,
  })

  /** Projects to choose from, rather than keys typed from memory. */
  const projects = useQuery({
    queryKey: ['jira', 'projects'],
    queryFn: ipc.jiraProjects,
    enabled: !!status.data?.connected && !!status.data?.site,
    retry: false,
  })

  /**
   * Every event source in the active registry, so a rule is picked rather than
   * remembered. Patterns stay free text — a rule can name a source that has
   * not published yet.
   */
  const { envId } = useSettings()
  const schemas = useQuery({
    queryKey: ['schemas', envId, 'list'],
    queryFn: () => ipc.listSchemas(envId),
    enabled: !!envId,
    retry: false,
  })

  /**
   * Sources seen on the bus, including ones nobody has written a schema for.
   *
   * The registry cannot supply those, and they are the ones most worth a rule
   * — a producer with no schema is exactly the one about to need a ticket.
   */
  const traffic = useQuery({
    queryKey: ['eventSources', envId],
    queryFn: () => ipc.eventSources(envId),
    enabled: !!envId,
    retry: false,
  })

  /**
   * Every source a rule could name, in the spelling a rule must use.
   *
   * A source containing a space is registered — and ticketed — under a dashed
   * spelling, so `Atomic Forms` has to be offered as `Atomic-Forms` with the
   * original shown alongside, or the rule would silently never match.
   */
  const patternOptions = useMemo(() => {
    const seen = new Map<string, string | undefined>()
    for (const entry of traffic.data ?? []) {
      seen.set(entry.pattern, entry.pattern === entry.source ? undefined : entry.source)
    }
    for (const schema of schemas.data ?? []) {
      if (schema.source && !seen.has(schema.source)) seen.set(schema.source, undefined)
    }

    const sources = [...seen.keys()].sort()
    return suggestPatterns(sources).map((pattern) => ({
      pattern,
      // What the events actually carry, when it differs from what a rule says.
      seenAs: seen.get(pattern),
    }))
  }, [traffic.data, schemas.data])

  /**
   * Issue types per project in play.
   *
   * Every project can name its types differently, so "Bug" is a guess until
   * the project says otherwise — and a wrong guess is a 400 at filing time.
   */
  const projectKeys = useMemo(
    () =>
      [
        ...new Set(
          [
            ...draft.jira.routes.map((route) => route.projectKey),
            ...draft.jira.ownerRoutes.map((route) => route.projectKey),
            draft.jira.defaultProject ?? '',
          ].filter(Boolean),
        ),
      ].sort(),
    [draft.jira.routes, draft.jira.ownerRoutes, draft.jira.defaultProject],
  )

  const issueTypeQueries = useQueries({
    queries: projectKeys.map((key) => ({
      queryKey: ['jira', 'issueTypes', key],
      queryFn: () => ipc.jiraIssueTypes(key),
      enabled: !!status.data?.connected && !!status.data?.site,
      retry: false,
      staleTime: 5 * 60_000,
    })),
  })

  const issueTypesByProject = useMemo(() => {
    const map = new Map<string, string[]>()
    projectKeys.forEach((key, i) => {
      const data = issueTypeQueries[i]?.data
      if (data) map.set(key, data)
    })
    return map
  }, [projectKeys, issueTypeQueries])


  const defaultTypes = draft.jira.defaultProject
    ? issueTypesByProject.get(draft.jira.defaultProject)
    : undefined

  return (
    <>
      <Panel title="Jira connection" bodyClassName="flex flex-col gap-3 p-2">
        {status.isLoading && <Spinner label="Checking Jira…" />}

        {status.data && (
          <div className="flex flex-wrap items-center gap-2">
            {status.data.connected ? (
              <Badge tone="ok">
                <CheckCircle2 className="size-2.5" />
                connected
              </Badge>
            ) : (
              <Badge tone="neutral">not connected</Badge>
            )}
            {status.data.site?.url && (
              <a
                href={status.data.site.url}
                target="_blank"
                rel="noreferrer"
                className="inline-flex items-center gap-1 text-[11px] text-accent hover:underline"
              >
                {status.data.site.name ?? status.data.site.url}
                <ExternalLink className="size-2.5" />
              </a>
            )}
            {status.data.detail && (
              <span className="text-[10px] text-ink-faint">{status.data.detail}</span>
            )}
          </div>
        )}

        {/*
          Says the quiet part: this cannot work until someone registers an app,
          and Atlassian's flow gives no way around the secret.
        */}
        <Note tone="info">
          <span>
            Atlassian supports only the authorization-code grant, so Pontifex needs a
            client ID <em>and</em> secret from an app registered in the{' '}
            <a
              className="underline"
              href="https://developer.atlassian.com/console/myapps/"
              target="_blank"
              rel="noreferrer"
            >
              developer console
            </a>
            . Set its callback URL to{' '}
            <span className="font-mono text-ink">{draft.jira.callbackUrl}</span> and grant
            it <span className="font-mono text-ink">read:jira-work</span>,{' '}
            <span className="font-mono text-ink">write:jira-work</span>,{' '}
            <span className="font-mono text-ink">read:jira-user</span> and{' '}
            <span className="font-mono text-ink">offline_access</span>. The secret is
            stored in your OS keychain, never in settings.json.
          </span>
        </Note>

        {/* The shortcut past all of the above: the console generates a URL
            containing every value, so paste it rather than transcribing. */}
        <Field
          label="Paste the authorization URL"
          hint="From Authorization → OAuth 2.0 (3LO) in the console — fills in the fields below"
        >
          <Input
            placeholder="https://auth.atlassian.com/authorize?audience=api.atlassian.com&client_id=…"
            spellCheck={false}
            onChange={(e) => void readUrl(e.target.value)}
          />
        </Field>

        {pasteError && <ErrorBox error={pasteError} />}

        {pasted &&
          (pasted.missingScopes.length > 0 ? (
            <Note tone="warn">
              <span>
                Read the client ID and callback, but the app is missing{' '}
                {pasted.missingScopes.map((scope, i) => (
                  <span key={scope}>
                    {i > 0 && ', '}
                    <span className="font-mono text-ink">{scope}</span>
                  </span>
                ))}
                . Add {pasted.missingScopes.length === 1 ? 'it' : 'them'} under Permissions
                → Jira API → Configure (classic scopes), then copy the URL again.
              </span>
            </Note>
          ) : (
            <Note tone="ok">
              Client ID, callback and all {pasted.scopes.length} scopes read from the URL.
              Add the secret and save.
            </Note>
          ))}

        <div className="grid grid-cols-[2fr_2fr] gap-2">
          <Field label="Client ID">
            <Input
              value={clientId}
              onChange={(e) => setClientId(e.target.value)}
              placeholder="from the developer console"
              className="font-mono"
              spellCheck={false}
            />
          </Field>
          <Field
            label="Client secret"
            hint={status.data?.configured ? 'stored — leave blank to keep' : 'stored in the keychain'}
          >
            <Input
              type="password"
              value={secret}
              onChange={(e) => setSecret(e.target.value)}
              placeholder={status.data?.configured ? '••••••••' : 'paste once'}
              spellCheck={false}
            />
          </Field>
        </div>

        <Field
          label="Callback URL"
          hint="used verbatim — must be the URL registered for the app"
        >
          <Input
            value={draft.jira.callbackUrl}
            onChange={(e) => patchJira({ callbackUrl: e.target.value })}
            className="font-mono"
            spellCheck={false}
          />
        </Field>

        {saveApp.isError && <ErrorBox error={saveApp.error} />}
        {connect.isError && <ErrorBox error={connect.error} />}

        <div className="flex items-center gap-2">
          <Button
            variant="secondary"
            loading={saveApp.isPending}
            disabled={!clientId.trim()}
            onClick={() => saveApp.mutate()}
          >
            Save app
          </Button>
          <Button
            variant="primary"
            loading={connect.isPending}
            disabled={!status.data?.configured}
            title={
              status.data?.configured
                ? 'Opens your browser to sign in through your work account'
                : 'Save the client ID and secret first'
            }
            onClick={() => connect.mutate()}
          >
            {status.data?.connected ? 'Reconnect' : 'Connect'}
          </Button>
          {connect.isPending && (
            <span className="text-[10px] text-ink-faint">
              Finish signing in in your browser…
            </span>
          )}
          {status.data?.connected && (
            <Button
              variant="ghost"
              className="ml-auto"
              loading={disconnect.isPending}
              onClick={() => disconnect.mutate()}
            >
              <Unplug className="size-3" />
              Disconnect
            </Button>
          )}
        </div>

        {/* Only a choice when there is one: a single-site grant selects itself. */}
        {status.data?.connected && (sites.data?.length ?? 0) > 1 && (
          <Field label="Site" hint="where tickets are filed">
            <Select
              value={status.data.site?.cloudId ?? ''}
              onChange={(e) => selectSite.mutate(e.target.value)}
            >
              <option value="">choose…</option>
              {sites.data?.map((site) => (
                <option key={site.id} value={site.id}>
                  {site.name} — {site.url}
                </option>
              ))}
            </Select>
          </Field>
        )}
      </Panel>

      <Panel
        title="Which team hears about which producer"
        bodyClassName="flex flex-col gap-2 p-2"
        actions={
          <Button
            variant="ghost"
            size="sm"
            onClick={() =>
              patchJira({
                routes: [...draft.jira.routes, blankRoute()],
              })
            }
          >
            <Plus className="size-3" />
            Rule
          </Button>
        }
      >
        <p className="px-1 text-[10px] text-ink-faint">
          Matched against the event source, with <span className="font-mono">*</span> as
          the only wildcard. The most specific matching rule wins, so{' '}
          <span className="font-mono">billing-invoices</span> beats{' '}
          <span className="font-mono">billing-*</span> beats{' '}
          <span className="font-mono">*</span>. Suggestions come from sampled traffic and
          the registry — a source whose name a schema cannot hold, like{' '}
          <span className="font-mono">Atomic Forms</span>, is matched by the spelling it
          is registered under.
        </p>

        <RouteRows
          routes={draft.jira.routes}
          emptyText="No rules yet — everything goes to the default project below."
          patternHeader="Source pattern"
          patternPlaceholder="billing-*"
          // A datalist, not a select: the list is a shortcut, and a rule may
          // legitimately name a source that has not published anything yet.
          patternListId="pontifex-source-patterns"
          projects={projects.data}
          issueTypesByProject={issueTypesByProject}
          defaultIssueType={draft.jira.issueType}
          onChange={(routes) => patchJira({ routes })}
        />

        <datalist id="pontifex-source-patterns">
          {patternOptions.map(({ pattern, seenAs }) => (
            <option key={pattern} value={pattern}>
              {seenAs && `events carry “${seenAs}”`}
            </option>
          ))}
        </datalist>

        <div className="mt-2 grid grid-cols-2 gap-2 border-t border-edge pt-2">
          <Field
            label="Default project"
            hint="for a source no rule matches — without one, those cannot be filed at all"
          >
            <ProjectPicker
              value={draft.jira.defaultProject ?? ''}
              projects={projects.data}
              emptyLabel="none — unmatched sources cannot be filed"
              onChange={(key) => patchJira({ defaultProject: key || null })}
            />
          </Field>
          <Field label="Default issue type" hint="used by any rule that does not override it">
            <IssueTypePicker
              value={draft.jira.issueType}
              types={defaultTypes}
              fallbackLabel="Bug"
              onChange={(type) => patchJira({ issueType: type })}
            />
          </Field>
        </div>

        {/* Say why the picker is a text box. Falling back silently left the
            impression that pontifex simply does not offer a project list. */}
        {!projects.data && (
          <p className="px-1 text-[10px] text-warn">
            {projects.isError
              ? `Could not read your Jira projects (${ipc.asIpcError(projects.error).message}), so keys have to be typed by hand and are unverified.`
              : !status.data?.connected
                ? 'Connect to Jira above to choose projects and issue types from your site — until then these are free text and unverified.'
                : 'Pick a Jira site above to choose projects from it.'}
          </p>
        )}
        {schemas.isError && (
          <p className="px-1 text-[10px] text-warn">
            Could not read the registry, so there are no source suggestions — patterns
            still work typed out.
          </p>
        )}
        {patternOptions.length === 0 && !schemas.isError && (
          <p className="px-1 text-[10px] text-ink-faint">
            No sources to suggest yet — run the Health report once and the sources seen on
            the bus, registered or not, will appear here.
          </p>
        )}

        <p className="flex items-center gap-1 px-1 text-[10px] text-ink-faint">
          <Bug className="size-2.5" />
          Every ticket is labelled so the same problem is never filed twice.
        </p>
      </Panel>

      <Panel
        title="Which team owns the code, when no source rule says"
        bodyClassName="flex flex-col gap-2 p-2"
        actions={
          <Button
            variant="ghost"
            size="sm"
            onClick={() =>
              patchJira({
                ownerRoutes: [...draft.jira.ownerRoutes, blankRoute()],
              })
            }
          >
            <Plus className="size-3" />
            Rule
          </Button>
        }
      >
        <p className="px-1 text-[10px] text-ink-faint">
          Matched against what the origin lookup found for the event type: the CODEOWNERS
          team, like <span className="font-mono">@acme/payments</span>, or the repository,
          like <span className="font-mono">acme/billing-*</span>. Consulted only when no
          source rule matches, and only for types whose origin has been looked up — the
          Origin tab on a schema, or a watch hit, does that.
        </p>

        <RouteRows
          routes={draft.jira.ownerRoutes}
          emptyText="No owner rules — a source no rule matches goes to the default project."
          patternHeader="Team or repository pattern"
          patternPlaceholder="@acme/payments"
          projects={projects.data}
          issueTypesByProject={issueTypesByProject}
          defaultIssueType={draft.jira.issueType}
          onChange={(ownerRoutes) => patchJira({ ownerRoutes })}
        />
      </Panel>
    </>
  )
}
