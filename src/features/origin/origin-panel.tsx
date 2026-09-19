import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { ExternalLink, GitCommitHorizontal, RefreshCw, Users } from 'lucide-react'
import { openUrl } from '@tauri-apps/plugin-opener'
import * as ipc from '@/lib/ipc'
import type { Authorship, EventOrigin, IpcError, ProducerOrigin, SkippedMatch } from '@/lib/types'
import { Badge, Button, ErrorBox, Note, Spinner, cn } from '@/components/ui'
import { formatAge } from '@/lib/format'

/**
 * Who to talk to about an event type.
 *
 * The registry says what an event looks like and nothing about who sends it,
 * so when a payload is wrong the schema is silent on whom to ask. This panel
 * answers with two things: the commit and pull request that wired the type
 * into the bus repo, and the code in the organisation that publishes it —
 * who first put it there, in which pull request, and who owns that code now.
 */
export function OriginPanel({ schemaName, compact = false }: { schemaName: string; compact?: boolean }) {
  const queryClient = useQueryClient()
  const origin = useQuery<EventOrigin, IpcError>({
    queryKey: ['origin', schemaName],
    queryFn: () => ipc.eventOrigin(schemaName),
    staleTime: Infinity,
    retry: false,
  })
  // A mutation rather than a bare call, so the button can show the ten or so
  // seconds a fresh search takes, and say so if it fails.
  const refresh = useMutation<EventOrigin, IpcError, number | void>({
    mutationFn: (limit) => ipc.eventOrigin(schemaName, true, limit ?? undefined),
    onSuccess: (fresh) => queryClient.setQueryData(['origin', schemaName], fresh),
  })
  // One mutation serves both buttons; only the one that was pressed spins.
  const examining = refresh.isPending && typeof refresh.variables === 'number'
  const refreshing = refresh.isPending && !examining

  if (origin.isLoading) return <Spinner label="Searching for where this event type came from…" />
  if (origin.isError) return <ErrorBox error={origin.error} className="m-3" />
  if (!origin.data) return null
  const data = origin.data
  const producers = data.producers
  const groups = groupByRepo(producers)
  /** Matches the search found that were neither ignored nor read. */
  const unread = data.github.totalMatches - data.github.skipped - producers.length

  return (
    <div className={cn('flex flex-col gap-3', compact ? 'p-2' : 'p-3')}>
      <section className="flex flex-col gap-1">
        <h3 className="text-[11px] font-semibold text-ink">Wired into the bus</h3>
        {data.wiring ? (
          <AuthorshipLine who={data.wiring} repoUrl={data.wiringRepoUrl} verb="wired in" />
        ) : (
          <p className="text-[11px] text-ink-faint">
            No commit in the local global-event-bus checkout mentions{' '}
            <span className="font-mono">{data.detailType}</span>. Either the checkout is behind, or
            the bus matches this type by pattern rather than by name.
          </p>
        )}
      </section>

      {data.github.status !== 'ok' && (
        <Note tone={data.github.status === 'error' ? 'danger' : 'warn'}>{data.github.message}</Note>
      )}
      {data.github.status === 'ok' && producers.length === 0 && (
        <section className="flex flex-col gap-1">
          <h3 className="text-[11px] font-semibold text-ink">Published from</h3>
          <p className="text-[11px] text-ink-faint">
            Nothing in {data.github.org} carries the literal{' '}
            <span className="font-mono">{data.detailType}</span>. The producer may build the name
            from parts, live outside the organisation, or be on a branch the search does not index.
          </p>
        </section>
      )}
      {ROLE_SECTIONS.map(({ role, title, empty }) => {
        const ofRole = groups.filter((g) => g.role === role)
        // A section with nothing in it is shown only when it has something to
        // say about that — and only once there was something to sort.
        if (ofRole.length === 0 && !(empty && producers.length > 0)) return null
        return (
          <section key={role} className="flex flex-col gap-1.5">
            <h3 className="text-[11px] font-semibold text-ink">{title}</h3>
            {ofRole.length === 0 && <p className="text-[11px] text-ink-faint">{empty}</p>}
            {ofRole.map((group) => (
              <RepoCard key={group.repo} group={group} />
            ))}
          </section>
        )
      })}
      {data.github.status === 'ok' && data.github.skipped > 0 && (
        <SkippedList skipped={data.github.skippedFiles ?? []} count={data.github.skipped} />
      )}
      {data.github.status === 'ok' && unread > 0 && (
        // The split only sorts what was read; an unread match could still be
        // the publisher. Reading the rest is a fresh, longer lookup; the
        // backend caps it at what one search returns.
        <Button
          variant="ghost"
          size="sm"
          className="self-start"
          onClick={() => refresh.mutate(data.github.totalMatches)}
          loading={examining}
          disabled={refreshing}
          title="Read every matching file, at a couple of seconds each"
        >
          {examining ? 'Searching…' : `Examine the ${unread} more match${unread === 1 ? '' : 'es'}`}
        </Button>
      )}

      <div className="flex items-center gap-2 text-[10px] text-ink-faint">
        <span>looked up {formatAge(Date.now() - data.cachedAt)} ago</span>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => refresh.mutate()}
          loading={refreshing}
          disabled={examining}
          title="Search again now — a fresh lookup takes about ten seconds"
        >
          {!refreshing && <RefreshCw className="size-3" />}
          {refreshing ? 'Searching…' : 'Refresh'}
        </Button>
      </div>
      {refresh.isError && <ErrorBox error={refresh.error} />}
    </div>
  )
}

type Role = ProducerOrigin['role']

/**
 * Publishers first — the answer to "who do I talk to" — then who listens,
 * then places that merely name the type.
 */
const ROLE_SECTIONS: { role: Role; title: string; empty?: string }[] = [
  {
    role: 'publisher',
    title: 'Published by',
    empty: 'No file was recognised as publishing this type — nothing calls PutEvents with it, and no repo is named after its source.',
  },
  { role: 'consumer', title: 'Consumed by' },
  { role: 'mention', title: 'Also mentioned in' },
]

const ROLE_RANK: Record<Role, number> = { publisher: 2, consumer: 1, mention: 0 }

/** The organisation's files that carry the type, gathered per repository. */
interface RepoGroup {
  repo: string
  /** Every file, led by the one that earned the repo its role. */
  files: ProducerOrigin[]
  owners: string[]
  /** The strongest role among the repo's files. */
  role: Role
}

/**
 * A literal shows up in every file that names it — a constants module, an
 * OpenAPI spec, a fixture, two tests — and that is one service, not five
 * places to look. One card per repository, led by the earliest introduction,
 * is the answer to "who"; the file list is there for "where exactly".
 */
function groupByRepo(producers: ProducerOrigin[]): RepoGroup[] {
  const byRepo = new Map<string, ProducerOrigin[]>()
  for (const p of producers) byRepo.set(p.repo, [...(byRepo.get(p.repo) ?? []), p])
  const dated = (p: ProducerOrigin) => p.introduced?.date ?? '~'
  const groups: RepoGroup[] = []
  for (const [repo, files] of byRepo) {
    const sorted = [...files].sort(
      (a, b) => Number(a.incidental) - Number(b.incidental) || dated(a).localeCompare(dated(b)),
    )
    const role = sorted.reduce<Role>(
      (best, p) => (ROLE_RANK[p.role] > ROLE_RANK[best] ? p.role : best),
      'mention',
    )
    // Lead with the file that earned the repo its role, not with a test.
    const lead = sorted.find((p) => p.role === role && !p.incidental) ?? sorted[0]
    groups.push({
      repo,
      files: [lead, ...sorted.filter((p) => p !== lead)],
      owners: [...new Set(sorted.flatMap((p) => p.owners))],
      role,
    })
  }
  // Repositories with a real producer first, then by when it first appeared.
  return groups.sort(
    (a, b) =>
      Number(a.files[0].incidental) - Number(b.files[0].incidental) ||
      dated(a.files[0]).localeCompare(dated(b.files[0])),
  )
}

function RepoCard({ group }: { group: RepoGroup }) {
  const [showAll, setShowAll] = useState(false)
  const [owner, repo] = group.repo.split('/')
  const [lead, ...others] = group.files
  return (
    <div
      className={cn(
        'flex flex-col gap-1 rounded-md border border-edge bg-surface-0 px-2 py-1.5',
        lead.incidental && 'opacity-70',
      )}
    >
      <div className="flex items-center gap-2 text-[11px]">
        <span className="whitespace-nowrap font-medium text-ink">{repo ?? owner}</span>
        {lead.incidental && (
          <Badge tone="neutral" title="Only tests, fixtures or documents mention it here">
            incidental
          </Badge>
        )}
        {group.owners.length > 0 && (
          <span
            className="ml-auto flex min-w-0 items-center gap-1 truncate text-ink-muted"
            title={`From CODEOWNERS: ${group.owners.join(' ')}`}
          >
            <Users className="size-3 shrink-0" />
            <span className="truncate">{group.owners.join(' ')}</span>
          </span>
        )}
      </div>
      {lead.introduced ? (
        <AuthorshipLine who={lead.introduced} verb="first published" />
      ) : (
        <p className="text-[10px] text-ink-faint">
          The commit that introduced it was not found in the history read.
        </p>
      )}
      <FileLink file={lead} />
      {others.length > 0 && (
        <>
          {showAll && others.map((f) => <FileLink key={f.path} file={f} secondary />)}
          <button
            type="button"
            onClick={() => setShowAll((v) => !v)}
            className="self-start text-[10px] text-ink-faint hover:text-ink hover:underline"
          >
            {showAll
              ? 'fewer files'
              : `${others.length} more file${others.length === 1 ? '' : 's'} in this repo`}
          </button>
        </>
      )}
    </div>
  )
}

/**
 * What the ignore list kept out, with the pattern responsible for each —
 * the way to notice a pattern that is broader than it was meant to be.
 */
function SkippedList({ skipped, count }: { skipped: SkippedMatch[]; count: number }) {
  const [open, setOpen] = useState(false)
  return (
    <div className="flex flex-col gap-0.5">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="self-start text-[10px] text-ink-faint hover:text-ink hover:underline"
        title="Set in Settings → Repo"
      >
        {count} match{count === 1 ? '' : 'es'} skipped by the ignore list {open ? '▾' : '▸'}
      </button>
      {open &&
        skipped.map((s) => (
          <div
            key={`${s.repo}/${s.path}`}
            className="flex min-w-0 items-center gap-2 pl-3 text-[10px]"
          >
            <button
              type="button"
              onClick={() => void openUrl(s.url)}
              className="flex min-w-0 items-center gap-1 text-ink-muted hover:underline"
              title={s.url}
            >
              <span className="shrink-0">{s.repo.split('/')[1] ?? s.repo}</span>
              <span className="truncate font-mono">{s.path}</span>
              <ExternalLink className="size-3 shrink-0 text-ink-faint" />
            </button>
            <span className="ml-auto shrink-0 font-mono text-ink-faint" title="Matched by">
              {s.pattern}
            </span>
          </div>
        ))}
    </div>
  )
}

/** One file that carries the type, as a link, with its own introduction when secondary. */
function FileLink({ file, secondary = false }: { file: ProducerOrigin; secondary?: boolean }) {
  return (
    <div className={cn('flex min-w-0 flex-col gap-0.5', secondary && 'pl-3')}>
      <button
        type="button"
        onClick={() => void openUrl(file.url)}
        className="flex min-w-0 items-center gap-1 text-left text-[10px] text-ink-muted hover:underline"
        title={file.url}
      >
        <span className="truncate font-mono">{file.path}</span>
        <ExternalLink className="size-3 shrink-0 text-ink-faint" />
        {file.incidental && <span className="shrink-0 text-ink-faint">· incidental</span>}
      </button>
      {secondary && file.introduced && (
        <span className="text-[10px] text-ink-faint">
          added by {file.introduced.author}
          {file.introduced.pullNumber ? ` in PR #${file.introduced.pullNumber}` : ''}
        </span>
      )}
    </div>
  )
}

/** "first published by Name on 3 Mar 2026 in PR #42 Title". */
function AuthorshipLine({ who, verb, repoUrl }: { who: Authorship; verb: string; repoUrl?: string | null }) {
  const when = new Date(who.date)
  const date = Number.isNaN(when.getTime())
    ? who.date
    : when.toLocaleDateString([], { year: 'numeric', month: 'short', day: 'numeric' })
  const pullUrl =
    who.pullUrl ?? (who.pullNumber && repoUrl ? `${repoUrl}/pull/${who.pullNumber}` : null)
  const commitUrl = who.commitUrl ?? (repoUrl ? `${repoUrl}/commit/${who.sha}` : null)
  return (
    <p className="flex flex-wrap items-center gap-x-1 text-[11px] text-ink-muted">
      <GitCommitHorizontal className="size-3 text-ink-faint" />
      {verb} by <span className="text-ink">{who.author}</span>
      {who.login && <span className="text-ink-faint">(@{who.login})</span>}
      on {date}
      {who.pullNumber ? (
        <>
          {' in '}
          <button
            type="button"
            onClick={() => pullUrl && void openUrl(pullUrl)}
            className={cn('font-mono', pullUrl && 'hover:underline')}
            title={who.pullTitle ?? undefined}
          >
            PR #{who.pullNumber}
          </button>
          {who.pullTitle && <span className="truncate text-ink-faint">{who.pullTitle}</span>}
        </>
      ) : (
        <>
          {' — '}
          <button
            type="button"
            onClick={() => commitUrl && void openUrl(commitUrl)}
            className={cn('truncate', commitUrl && 'hover:underline')}
            title={who.sha}
          >
            {who.subject}
          </button>
        </>
      )}
    </p>
  )
}
