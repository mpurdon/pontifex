import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'
import {
  AlertTriangle,
  ArrowRight,
  ChevronDown,
  ChevronRight,
  GitFork,
  RefreshCw,
} from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type { LiveBus } from '@/lib/types'
import {
  Badge,
  Button,
  EmptyState,
  ErrorBox,
  Note,
  Panel,
  Spinner,
  Toolbar,
  cn,
} from '@/components/ui'
import { useSettings } from '@/app/settings-context'
import { useLoginForEnvironment } from '@/app/login-dialog'

export function TopologyPage() {
  const { envId, activeEnvironment } = useSettings()
  const credentials = useLoginForEnvironment(activeEnvironment)
  const [expanded, setExpanded] = useState<Set<string>>(new Set())

  const topology = useQuery({
    queryKey: ['topology', envId],
    queryFn: () => ipc.getTopology(envId),
    enabled: !!envId,
    retry: false,
  })

  const toggle = (name: string) => {
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(name)) next.delete(name)
      else next.add(name)
      return next
    })
  }

  if (topology.isLoading) return <Spinner label="Reading bus topology…" />
  if (topology.isError) {
    return (
      <div className="p-3">
        <ErrorBox error={ipc.asIpcError(topology.error)} {...credentials} />
      </div>
    )
  }
  if (!topology.data) return null

  const { declared, live, drift, configWarning, configPath, accountId } = topology.data

  return (
    <div className="flex h-full flex-col">
      <Toolbar>
        <span className="text-xs font-semibold text-ink">
          {topology.data.stage}
        </span>
        <span className="font-mono text-[10px] text-ink-faint">
          {accountId ?? '—'} · {topology.data.region}
        </span>
        <div className="ml-auto flex items-center gap-2">
          <Badge tone="neutral">{declared.length} declared</Badge>
          <Badge tone="neutral">{live.length} live</Badge>
          {drift.declaredOnly.length > 0 && (
            <Badge tone="warn">{drift.declaredOnly.length} not visible</Badge>
          )}
          {drift.liveOnly.length > 0 && (
            <Badge tone="info">{drift.liveOnly.length} undeclared</Badge>
          )}
          <Button
            variant="ghost"
            size="sm"
            onClick={() => topology.refetch()}
            title="Refresh"
          >
            <RefreshCw
              className={topology.isFetching ? 'size-3 animate-spin' : 'size-3'}
            />
          </Button>
        </div>
      </Toolbar>

      <div className="grid min-h-0 flex-1 grid-cols-[380px_1fr] gap-3 overflow-hidden p-3">
        <div className="flex min-h-0 flex-col gap-3">
          <Panel
            title={`Declared sources (${declared.length})`}
            bodyClassName="p-2"
            className="min-h-0 flex-1"
          >
            {configWarning && (
              <Note className="mb-2">
                {configWarning}
                {configPath && (
                  <span className="mt-1 block font-mono text-[10px] opacity-70">
                    {configPath}
                  </span>
                )}
              </Note>
            )}

            {declared.length === 0 ? (
              <p className="p-2 text-xs text-ink-faint">
                Nothing declared for this stage.
              </p>
            ) : (
              <ul className="flex flex-col gap-1">
                {declared.map((bus) => {
                  const missing = drift.declaredOnly.some(
                    (d) => d.sourceBusName === bus.sourceBusName,
                  )
                  const crossAccount = !!accountId && bus.accountId !== accountId
                  return (
                    <li
                      key={`${bus.category}-${bus.sourceBusName}`}
                      className="rounded-md border border-edge bg-surface-2 px-2 py-1.5"
                    >
                      <div className="flex items-center gap-1.5">
                        <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-ink">
                          {bus.sourceBusName}
                        </span>
                        {bus.category === 'thirdParty' && (
                          <Badge tone="info">3P</Badge>
                        )}
                        {crossAccount && (
                          <Badge tone="neutral" title={bus.sourceArn}>
                            {bus.accountId}
                          </Badge>
                        )}
                        {missing && (
                          <Badge
                            tone="warn"
                            title={
                              crossAccount
                                ? 'Cross-account bus — expected when this profile cannot read the owning account'
                                : 'Declared but not found in this account'
                            }
                          >
                            <AlertTriangle className="size-2.5" />
                          </Badge>
                        )}
                      </div>
                    </li>
                  )
                })}
              </ul>
            )}
          </Panel>

          {drift.liveOnly.length > 0 && (
            <Panel
              title={`Undeclared buses (${drift.liveOnly.length})`}
              bodyClassName="p-2"
              className="max-h-48 shrink-0"
            >
              <p className="mb-2 px-1 text-[10px] text-ink-faint">
                Present in the account but absent from busConfiguration.ts.
              </p>
              <ul className="flex flex-col gap-0.5">
                {drift.liveOnly.map((name) => (
                  <li
                    key={name}
                    className="truncate px-1 font-mono text-[11px] text-ink-muted"
                  >
                    {name}
                  </li>
                ))}
              </ul>
            </Panel>
          )}
        </div>

        <Panel title={`Live buses (${live.length})`} bodyClassName="p-2">
          {live.length === 0 ? (
            <EmptyState
              icon={<GitFork className="size-8" />}
              title="No event buses visible"
              detail="The selected profile may not have events:ListEventBuses in this account."
            />
          ) : (
            <ul className="flex flex-col gap-1.5">
              {live.map((bus) => (
                <BusCard
                  key={bus.name}
                  bus={bus}
                  expanded={expanded.has(bus.name)}
                  onToggle={() => toggle(bus.name)}
                />
              ))}
            </ul>
          )}
        </Panel>
      </div>
    </div>
  )
}

function BusCard({
  bus,
  expanded,
  onToggle,
}: {
  bus: LiveBus
  expanded: boolean
  onToggle: () => void
}) {
  const disabledRules = bus.rules.filter((r) => r.state === 'DISABLED').length

  return (
    <li className="overflow-hidden rounded-md border border-edge bg-surface-2">
      <button
        type="button"
        onClick={onToggle}
        className="flex w-full items-center gap-2 px-2 py-1.5 text-left hover:bg-surface-3"
      >
        {expanded ? (
          <ChevronDown className="size-3 shrink-0 text-ink-faint" />
        ) : (
          <ChevronRight className="size-3 shrink-0 text-ink-faint" />
        )}
        <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-ink">
          {bus.name}
        </span>
        <Badge tone="neutral">
          {bus.rules.length} rule{bus.rules.length === 1 ? '' : 's'}
        </Badge>
        {disabledRules > 0 && (
          <Badge tone="warn">{disabledRules} disabled</Badge>
        )}
      </button>

      {expanded && (
        <div className="border-t border-edge px-2 py-1.5">
          {bus.rules.length === 0 ? (
            <p className="text-[11px] text-ink-faint">No rules on this bus.</p>
          ) : (
            <ul className="flex flex-col gap-1.5">
              {bus.rules.map((rule) => (
                <li key={rule.name} className="text-[11px]">
                  <div className="flex items-center gap-1.5">
                    <span
                      className={cn(
                        'truncate font-mono',
                        rule.state === 'DISABLED'
                          ? 'text-ink-faint line-through'
                          : 'text-ink-muted',
                      )}
                    >
                      {rule.name}
                    </span>
                    {rule.state === 'DISABLED' && (
                      <Badge tone="warn">disabled</Badge>
                    )}
                  </div>

                  {rule.targets.length > 0 && (
                    <ul className="mt-0.5 flex flex-col gap-0.5 pl-3">
                      {rule.targets.map((target) => (
                        <li
                          key={target.id}
                          className="flex items-center gap-1 text-ink-faint"
                          title={target.arn}
                        >
                          <ArrowRight className="size-2.5 shrink-0" />
                          <span className="shrink-0 text-info">
                            {target.service}
                          </span>
                          <span className="truncate font-mono">
                            {target.arn.split(/[:/]/).pop()}
                          </span>
                        </li>
                      ))}
                    </ul>
                  )}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </li>
  )
}
