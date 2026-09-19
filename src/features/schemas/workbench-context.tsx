import { createContext, useContext, useMemo, useState, type ReactNode } from 'react'
import type { FilterStatus } from '@/features/report/status'

interface WorkbenchValue {
  /** The schema the Schemas screen is focused on. */
  selected: string | null
  select: (name: string | null) => void
  /** Whether the editor's side panel is showing Analysis. */
  analysisOpen: boolean
  setAnalysisOpen: (open: boolean) => void
  /** Whether the health report's detail charts are expanded. */
  chartsOpen: boolean
  setChartsOpen: (open: boolean) => void
  /** The health report's window, in minutes. */
  healthMinutes: number
  setHealthMinutes: (minutes: number) => void
  /** The health report's free-text filter. */
  healthFilter: string
  setHealthFilter: (filter: string) => void
  /** Which statuses the health report shows; empty means all. */
  healthStatuses: Set<FilterStatus>
  setHealthStatuses: (update: (prev: Set<FilterStatus>) => Set<FilterStatus>) => void
}

const WorkbenchContext = createContext<WorkbenchValue | null>(null)

/**
 * What you were doing, kept across screen changes.
 *
 * These were all `useState` inside their screens, so walking between the
 * Schemas and Health tabs threw them away: the selected schema reverted to
 * nothing, the issues panel closed itself, and a deliberately collapsed chart
 * drawer sprang back open. None of them is a property of a route — they are
 * how you have arranged your workspace — so they live above the router and
 * persist until you change them.
 *
 * Not persisted to disk: these are meaningful for a session, and restoring a
 * week-old arrangement on launch would be noise.
 */
export function WorkbenchProvider({ children }: { children: ReactNode }) {
  const [selected, select] = useState<string | null>(null)
  const [analysisOpen, setAnalysisOpen] = useState(false)
  // Open by default: the charts are the point of the screen until you have
  // read them, after which closing them is one click.
  const [chartsOpen, setChartsOpen] = useState(true)
  const [healthMinutes, setHealthMinutes] = useState(1440)
  const [healthFilter, setHealthFilter] = useState('')
  // Starts on `missing` alone: an event nobody has written a schema for is
  // the worst thing the report can find, and opening on all 300 rows buries
  // it. Kept here so opening a schema from the report and coming back finds
  // the chips as you left them.
  const [healthStatuses, setHealthStatuses] = useState<Set<FilterStatus>>(
    () => new Set(['missing']),
  )

  const value = useMemo(
    () => ({
      selected,
      select,
      analysisOpen,
      setAnalysisOpen,
      chartsOpen,
      setChartsOpen,
      healthMinutes,
      setHealthMinutes,
      healthFilter,
      setHealthFilter,
      healthStatuses,
      setHealthStatuses,
    }),
    [selected, analysisOpen, chartsOpen, healthMinutes, healthFilter, healthStatuses],
  )

  return <WorkbenchContext.Provider value={value}>{children}</WorkbenchContext.Provider>
}

export function useWorkbench(): WorkbenchValue {
  const context = useContext(WorkbenchContext)
  if (!context) throw new Error('useWorkbench must be used inside a WorkbenchProvider')
  return context
}
