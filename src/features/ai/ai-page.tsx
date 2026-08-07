import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { listen } from '@tauri-apps/api/event'
import { useEffect, useRef, useState } from 'react'
import { CheckCircle2, Save, Sparkles, Wand2, XCircle } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type { AiAction, AiResponse, IpcError } from '@/lib/types'
import {
  Badge,
  Button,
  EmptyState,
  ErrorBox,
  Field,
  FindingList,
  Input,
  Note,
  Panel,
  Select,
  Segmented,
} from '@/components/ui'
import { JsonEditor } from '@/components/lazy-editor'
import { stringify } from '@/lib/format'
import { useSettings } from '@/app/settings-context'
import { useLoginForProfileName } from '@/app/login-dialog'
import { useAiDraft } from './ai-draft-context'

const ACTIONS: { id: AiAction; label: string; placeholder: string }[] = [
  {
    id: 'generate',
    label: 'Generate',
    placeholder:
      'A packet notification was assigned to a reviewer. Carries the veteran id, the case record id, the assigned user id and an assignment timestamp.',
  },
  {
    id: 'refactor',
    label: 'Refactor',
    placeholder:
      'Add descriptions to every field, and add an optional `priority` enum of low | normal | urgent.',
  },
  {
    id: 'explain',
    label: 'Explain',
    placeholder: 'Optional: what to focus on.',
  },
]

export function AiPage() {
  const { settings, envId, activeEnvironment } = useSettings()
  const queryClient = useQueryClient()
  const credentials = useLoginForProfileName(settings?.llm.awsProfile ?? undefined)
  const { draft, setDraft } = useAiDraft()

  const [action, setAction] = useState<AiAction>(draft ? 'refactor' : 'generate')
  const [prompt, setPrompt] = useState('')
  const [source, setSource] = useState('')
  const [detailType, setDetailType] = useState('')
  const [modelId, setModelId] = useState('')
  const [streamed, setStreamed] = useState('')
  const [result, setResult] = useState<AiResponse | null>(null)
  const [edited, setEdited] = useState<string | null>(null)
  const streamRef = useRef<HTMLPreElement>(null)

  // Existing schemas double as few-shot examples, so generated output matches
  // the conventions actually in use rather than generic OpenAPI.
  const schemas = useQuery({
    queryKey: ['schemas', envId, 'list'],
    queryFn: () => ipc.listSchemas(envId),
    enabled: !!envId,
    retry: false,
  })

  useEffect(() => {
    const unlisten = listen<string>('bedrock://delta', (event) => {
      setStreamed((prev) => prev + event.payload)
    })
    return () => {
      void unlisten.then((fn) => fn())
    }
  }, [])

  useEffect(() => {
    streamRef.current?.scrollTo({ top: streamRef.current.scrollHeight })
  }, [streamed])

  const generate = useMutation<AiResponse, IpcError>({
    mutationFn: async () => {
      setStreamed('')
      setResult(null)
      setEdited(null)

      // Pull two real schemas to anchor the style. Cheap relative to the
      // model call, and only for the actions that produce a document.
      let examples: unknown[] = []
      if (action !== 'explain' && schemas.data?.length) {
        const picks = schemas.data.slice(0, 2)
        const details = await Promise.allSettled(
          picks.map((s) => ipc.describeSchema(s.name, undefined, envId)),
        )
        examples = details
          .filter((r) => r.status === 'fulfilled')
          .map((r) => (r as PromiseFulfilledResult<{ content: unknown }>).value.content)
      }

      return ipc.aiGenerate({
        action,
        prompt,
        schema: action === 'generate' ? undefined : draft?.content,
        source: source.trim() || undefined,
        detailType: detailType.trim() || undefined,
        modelId: modelId || undefined,
        examples,
      })
    },
    onSuccess: setResult,
  })

  const save = useMutation<unknown, IpcError>({
    mutationFn: () => {
      const content = JSON.parse(edited ?? stringify(result!.content))
      const name =
        result!.validation?.identity
          ? `${result!.validation.identity.source}@${result!.validation.identity.detailType}`
          : (draft?.name ?? '')
      if (!name) {
        throw { kind: 'invalid', message: 'Could not determine the schema name' } as IpcError
      }
      return ipc.putSchema(name, content, undefined, envId)
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['schemas', envId] })
    },
  })

  const hasModel = (settings?.llm.models.length ?? 0) > 0
  const hasProfile = !!settings?.llm.awsProfile
  const activeAction = ACTIONS.find((a) => a.id === action)!
  // `content` is `unknown`, so keep an explicit boolean for JSX conditionals.
  const content = result?.content
  const hasContent = content !== null && content !== undefined
  const text = edited ?? (hasContent ? stringify(content) : '')

  if (!hasProfile || !hasModel) {
    return (
      <EmptyState
        icon={<Sparkles className="size-8" />}
        title="AI generation is not configured yet"
        detail={
          !hasProfile
            ? 'Choose an AWS profile for Bedrock in Settings → AI.'
            : 'No Bedrock models configured. Import them from your Claude Code settings in Settings → AI, or list them from Bedrock.'
        }
      />
    )
  }

  return (
    <div className="grid h-full grid-cols-[380px_1fr] gap-3 p-3">
      <Panel title="Prompt" bodyClassName="flex flex-col gap-3 p-3">
        <Segmented
          size="lg"
          value={action}
          options={ACTIONS.map(({ id, label }) => ({
            id,
            label,
            disabled: id !== 'generate' && !draft,
            title:
              id !== 'generate' && !draft
                ? 'Open a schema and use "Send to AI" first'
                : undefined,
          }))}
          onChange={setAction}
        />

        {draft && action !== 'generate' && (
          <Note>
            Working on <span className="font-mono">{draft.name}</span>
            <button
              type="button"
              className="ml-2 underline"
              onClick={() => {
                setDraft(null)
                setAction('generate')
              }}
            >
              clear
            </button>
          </Note>
        )}

        {action === 'generate' && (
          <div className="grid grid-cols-2 gap-2">
            <Field label="Source" hint="optional — the model picks one if blank">
              <Input
                value={source}
                onChange={(e) => setSource(e.target.value)}
                placeholder="my-service"
                spellCheck={false}
              />
            </Field>
            <Field label="Detail type" hint="optional">
              <Input
                value={detailType}
                onChange={(e) => setDetailType(e.target.value)}
                placeholder="thing-happened"
                spellCheck={false}
              />
            </Field>
          </div>
        )}

        <Field label="Model">
          <Select value={modelId} onChange={(e) => setModelId(e.target.value)}>
            <option value="">
              {settings?.llm.models.find(
                (m) => m.id === settings.llm.selectedModelId,
              )?.label ?? settings?.llm.models[0]?.label ?? 'default'}
            </option>
            {settings?.llm.models.map((model) => (
              <option key={model.id} value={model.modelId}>
                {model.label}
              </option>
            ))}
          </Select>
        </Field>

        <label className="flex min-h-0 flex-1 flex-col gap-1">
          <span className="text-[11px] font-medium text-ink-muted">
            {action === 'explain' ? 'Focus (optional)' : 'Describe the change'}
          </span>
          <textarea
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            placeholder={activeAction.placeholder}
            className="min-h-32 flex-1 resize-none rounded-md border border-edge bg-surface-1 p-2 font-sans text-xs text-ink placeholder:text-ink-faint focus:border-accent focus:outline-none"
          />
        </label>

        <Button
          variant="primary"
          loading={generate.isPending}
          disabled={action !== 'explain' && !prompt.trim()}
          onClick={() => generate.mutate()}
        >
          <Wand2 className="size-3" />
          {activeAction.label}
        </Button>

        {generate.isError && <ErrorBox error={generate.error} {...credentials} />}
      </Panel>

      <Panel
        title={
          <span className="flex items-center gap-2">
            Result
            {result?.validation &&
              (result.validation.valid ? (
                <Badge tone="ok">
                  <CheckCircle2 className="size-2.5" />
                  valid
                </Badge>
              ) : (
                <Badge tone="danger">
                  <XCircle className="size-2.5" />
                  invalid
                </Badge>
              ))}
            {result && (
              <span className="font-mono text-[10px] font-normal text-ink-faint">
                {result.modelId.split('/').pop()}
              </span>
            )}
          </span>
        }
        actions={
          hasContent && (
            <Button
              variant="primary"
              size="sm"
              loading={save.isPending}
              disabled={!result?.validation?.valid}
              onClick={() => save.mutate()}
              title={
                result?.validation?.valid
                  ? `Register to ${activeEnvironment?.label}`
                  : 'Fix the validation errors first'
              }
            >
              <Save className="size-3" />
              Register
            </Button>
          )
        }
        bodyClassName="flex flex-col"
      >
        {save.isError && (
          <div className="p-3">
            <ErrorBox error={save.error} />
          </div>
        )}
        {save.isSuccess && (
          <div className="m-3 rounded-md border border-ok/40 bg-ok/10 px-3 py-2 text-xs text-ok">
            Registered to {activeEnvironment?.registryName}.
          </div>
        )}

        {generate.isPending || (streamed && !result) ? (
          <pre
            ref={streamRef}
            className="flex-1 overflow-auto whitespace-pre-wrap p-3 font-mono text-[11px] text-ink-muted"
          >
            {streamed || 'Waiting for the model…'}
          </pre>
        ) : result ? (
          hasContent ? (
            <>
              <div className="min-h-0 flex-1">
                <JsonEditor
                  value={text}
                  onChange={setEdited}
                  findings={result.validation?.findings ?? []}
                />
              </div>
              {result.validation && result.validation.findings.length > 0 && (
                <div className="max-h-28 shrink-0 overflow-auto border-t border-edge p-2">
                  <FindingList findings={result.validation.findings} />
                </div>
              )}
            </>
          ) : (
            // Explain, or a response we could not parse as JSON.
            <pre className="flex-1 overflow-auto whitespace-pre-wrap p-3 text-xs leading-relaxed text-ink-muted">
              {result.text}
            </pre>
          )
        ) : (
          <EmptyState
            icon={<Sparkles className="size-8" />}
            title="Nothing generated yet"
            detail="Results land here as an unsaved draft. Nothing is written to AWS until you register it."
          />
        )}
      </Panel>
    </div>
  )
}
