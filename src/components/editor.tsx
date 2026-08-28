import { DiffEditor, Editor, loader } from '@monaco-editor/react'
// Import the core API and the JSON language only. The package's default entry
// pulls in every language Monaco ships — TypeScript, HTML and CSS alone add
// ~9MB of workers this app has no use for.
import * as monaco from 'monaco-editor/editor/editor.api.js'
import 'monaco-editor/language/json/monaco.contribution.js'
// Monaco 0.56 exposes these through its exports map as `monaco-editor/<path>`,
// which resolves to `esm/vs/<path>`. Importing the `esm/vs/...` path directly
// double-applies that prefix and fails to resolve.
import editorWorker from 'monaco-editor/editor/editor.worker.js?worker'
import jsonWorker from 'monaco-editor/language/json/json.worker.js?worker'
import { useEffect, useMemo, useRef } from 'react'
import type { Finding } from '@/lib/types'

/**
 * Monaco setup for a desktop app.
 *
 * `@monaco-editor/react` fetches Monaco from a CDN by default, which cannot
 * work here: the app must run offline and the Tauri CSP forbids remote
 * scripts. Pointing the loader at the bundled instance and wiring the workers
 * through Vite's `?worker` imports keeps everything local.
 */
loader.config({ monaco })

self.MonacoEnvironment = {
  getWorker(_workerId: string, label: string) {
    return label === 'json' ? new jsonWorker() : new editorWorker()
  },
}

const THEME = 'pontifex-dark'

monaco.editor.defineTheme(THEME, {
  base: 'vs-dark',
  inherit: true,
  rules: [
    { token: 'string.key.json', foreground: 'a8c7fa' },
    { token: 'string.value.json', foreground: 'e8c07d' },
    { token: 'number', foreground: 'd19a66' },
    { token: 'keyword.json', foreground: 'c678dd' },
  ],
  colors: {
    'editor.background': '#1c1e26',
    'editorGutter.background': '#1c1e26',
    'editor.lineHighlightBackground': '#24262f',
    'editorLineNumber.foreground': '#5a5e70',
    'editorLineNumber.activeForeground': '#9aa0b5',
    'editorIndentGuide.background1': '#2a2d38',
    'diffEditor.insertedTextBackground': '#2ea04326',
    'diffEditor.removedTextBackground': '#f8514926',
  },
})

const baseOptions: monaco.editor.IStandaloneEditorConstructionOptions = {
  fontSize: 12,
  fontFamily:
    "ui-monospace, 'SF Mono', 'JetBrains Mono', Menlo, Consolas, monospace",
  minimap: { enabled: false },
  scrollBeyondLastLine: false,
  automaticLayout: true,
  tabSize: 2,
  renderWhitespace: 'none',
  smoothScrolling: true,
  padding: { top: 8, bottom: 8 },
  scrollbar: { verticalScrollbarSize: 10, horizontalScrollbarSize: 10 },
}

/**
 * Resolve a JSON Pointer to a line number in the *serialized* document.
 *
 * Monaco has no notion of JSON Pointers, so to place a validation marker we
 * walk the pointer segment by segment through the printed text, tracking how
 * far we have scanned. Approximate by design — it lands on the right key even
 * when a sibling shares the name — and a miss simply means no marker.
 */
function pointerToLine(text: string, pointer: string): number | null {
  if (!pointer || pointer === '/') return null

  const segments = pointer
    .split('/')
    .slice(1)
    .map((s) => s.replace(/~1/g, '/').replace(/~0/g, '~'))

  let searchFrom = 0
  let line: number | null = null

  for (const segment of segments) {
    // Array indices have no key to search for; skip them and keep the
    // position we have.
    if (/^\d+$/.test(segment)) continue

    const needle = `"${segment}"`
    const index = text.indexOf(needle, searchFrom)
    if (index === -1) return line
    searchFrom = index + needle.length
    line = text.slice(0, index).split('\n').length
  }

  return line
}

/** Turn validation findings into Monaco gutter markers. */
function applyMarkers(
  model: monaco.editor.ITextModel,
  findings: Finding[],
): void {
  const text = model.getValue()
  const markers: monaco.editor.IMarkerData[] = []

  for (const finding of findings) {
    const line = pointerToLine(text, finding.path)
    if (line === null) continue
    markers.push({
      severity:
        finding.severity === 'error'
          ? monaco.MarkerSeverity.Error
          : monaco.MarkerSeverity.Warning,
      message: finding.message,
      startLineNumber: line,
      startColumn: 1,
      endLineNumber: line,
      endColumn: model.getLineMaxColumn(line),
    })
  }

  monaco.editor.setModelMarkers(model, 'pontifex', markers)
}

export function JsonEditor({
  value,
  onChange,
  readOnly = false,
  findings = [],
  height = '100%',
}: {
  value: string
  onChange?: (value: string) => void
  readOnly?: boolean
  findings?: Finding[]
  height?: string | number
}) {
  const editorRef = useRef<monaco.editor.IStandaloneCodeEditor | null>(null)

  // Re-marking on every findings change keeps the gutter in step with the
  // validator, which runs debounced as the user types.
  useEffect(() => {
    const model = editorRef.current?.getModel()
    if (model) applyMarkers(model, findings)
  }, [findings, value])

  const options = useMemo(
    () => ({ ...baseOptions, readOnly, domReadOnly: readOnly }),
    [readOnly],
  )

  return (
    <Editor
      height={height}
      language="json"
      theme={THEME}
      value={value}
      options={options}
      onChange={(next) => onChange?.(next ?? '')}
      onMount={(editor) => {
        editorRef.current = editor
        const model = editor.getModel()
        if (model) applyMarkers(model, findings)
      }}
      loading={
        <div className="p-4 text-xs text-ink-faint">Loading editor…</div>
      }
    />
  )
}

export function JsonDiff({
  original,
  modified,
  originalLabel,
  modifiedLabel,
  height = '100%',
  renderSideBySide = true,
  onReachedEnd,
}: {
  original: string
  modified: string
  /** Caption for the left pane, e.g. "Live · v4". Both are optional; supply
   *  neither and the diff renders bare. */
  originalLabel?: string
  /** Caption for the right pane, e.g. "Your unsaved draft". */
  modifiedLabel?: string
  height?: string | number
  renderSideBySide?: boolean
  /**
   * Fired when the diff has been scrolled to the bottom — or immediately if it
   * is short enough not to scroll at all.
   *
   * Monaco scrolls inside its own viewport, so a wrapper's `onScroll` never
   * fires. Anything gating on "did you read this" has to ask the editor.
   */
  onReachedEnd?: () => void
}) {
  // `onMount` runs once, so it would capture the first render's callback.
  const reachedEnd = useRef(onReachedEnd)
  reachedEnd.current = onReachedEnd

  const diff = (
    <DiffEditor
      height={height}
      language="json"
      theme={THEME}
      original={original}
      modified={modified}
      options={{
        ...baseOptions,
        readOnly: true,
        renderSideBySide,
        // Whitespace-only differences are noise when comparing serialized JSON.
        ignoreTrimWhitespace: true,
      }}
      onMount={(editor) => {
        const pane = editor.getModifiedEditor()
        const check = () => {
          const visible = pane.getLayoutInfo().height
          const total = pane.getScrollHeight()
          // A diff that fits needs no scrolling, and demanding it anyway would
          // leave the gate permanently shut. The margin absorbs sub-pixel
          // layout rounding.
          if (pane.getScrollTop() + visible >= total - 4) reachedEnd.current?.()
        }
        pane.onDidScrollChange(check)
        // Layout is not settled on mount; re-check once it is.
        pane.onDidLayoutChange(check)
        check()
      }}
      loading={<div className="p-4 text-xs text-ink-faint">Loading diff…</div>}
    />
  )

  // Monaco renders two unlabelled panes, which leaves "which side is live and
  // which is mine" to be inferred from the content. The captions sit above the
  // panes and split on the same 50/50 the side-by-side diff uses.
  if (!originalLabel && !modifiedLabel) return diff

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 border-b border-edge bg-surface-1 text-[10px] font-semibold uppercase tracking-wide">
        <div className="flex-1 truncate border-r border-edge px-3 py-1 text-ink-muted">
          {originalLabel}
        </div>
        <div className="flex-1 truncate px-3 py-1 text-accent">{modifiedLabel}</div>
      </div>
      <div className="min-h-0 flex-1">{diff}</div>
    </div>
  )
}
