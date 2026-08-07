import { createContext, useContext, useMemo, useState, type ReactNode } from 'react'

interface AiDraft {
  name: string
  content: unknown
}

interface AiDraftContextValue {
  draft: AiDraft | null
  setDraft: (draft: AiDraft | null) => void
}

const AiDraftContext = createContext<AiDraftContextValue | null>(null)

/**
 * Carries a schema from the Schemas screen to the Generate screen.
 *
 * Kept in context rather than a route param because the payload is a whole
 * document, and round-tripping it through the URL would be lossy.
 */
export function AiDraftProvider({ children }: { children: ReactNode }) {
  const [draft, setDraft] = useState<AiDraft | null>(null)
  const value = useMemo(() => ({ draft, setDraft }), [draft])
  return <AiDraftContext.Provider value={value}>{children}</AiDraftContext.Provider>
}

export function useAiDraft(): AiDraftContextValue {
  const context = useContext(AiDraftContext)
  if (!context) throw new Error('useAiDraft must be used inside an AiDraftProvider')
  return context
}
