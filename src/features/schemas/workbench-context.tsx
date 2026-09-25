import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from 'react'
import type { FilterStatus } from '@/features/report/status'
import type { LogPage, LogQuery, WatchCondition } from '@/lib/types'

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
  /**
   * Rows you have marked done by hand, tied to the report they were marked
   * on. A re-run replaces every row, so marks from an older report do not
   * carry over to findings that may be new.
   */
  healthDone: HealthDone | null
  toggleHealthDone: (reportAt: number, name: string) => void
  /** Whether rows dealt with since the report are hidden rather than dimmed. */
  healthHideDone: boolean
  setHealthHideDone: (hide: boolean) => void
  /** What the Logs screen's filter bar is set to. */
  logFilters: LogFilters
  setLogFilters: (update: (prev: LogFilters) => LogFilters) => void
  /** The search the Logs screen last ran, with the pages read for it. */
  logRun: LogRun | null
  /** Run a search: the query becomes the current one, continued pages drop. */
  startLogRun: (envId: string, query: LogQuery) => void
  /** Add a page read by "continue further back" to the current run. */
  appendLogPage: (page: LogPage) => void
  /** Back to how the Logs screen opens: filters at their defaults, no run. */
  clearLogSearch: () => void
}

/** The Logs screen's filter bar. */
export interface LogFilters {
  logGroup: string
  minutes: number
  source: string
  detailType: string
  rawPattern: string
  useRaw: boolean
  /** Whether the payload-condition drawer is open. */
  advanced: boolean
  conditions: WatchCondition[]
  /**
   * What the last Search compiled `conditions` into. Kept with the filters so
   * coming back to the screen does not have to compile the same thing again.
   */
  compiledPattern: string | null
}

export const defaultLogFilters: LogFilters = {
  logGroup: '',
  minutes: 60,
  source: '',
  detailType: '',
  rawPattern: '',
  useRaw: false,
  advanced: false,
  conditions: [],
  compiledPattern: null,
}

/** A search that has been run, and everything read for it. */
export interface LogRun {
  /**
   * The environment it was run against. A run belongs to its environment, so
   * switching environments retires it rather than mixing two accounts' events.
   */
  envId: string
  query: LogQuery
  /** Pages from "continue further back", in the order they were read. */
  older: LogPage[]
}

export interface HealthDone {
  /** `RegistryReport.generatedAt` of the report these marks belong to. */
  reportAt: number
  names: Set<string>
}

const WorkbenchContext = createContext<WorkbenchValue | null>(null)

/**
 * What you were doing, kept across screen changes.
 *
 * These were all `useState` inside their screens, so walking between the
 * Schemas and Health tabs threw them away: the selected schema reverted to
 * nothing, the issues panel closed itself, a deliberately collapsed chart
 * drawer sprang back open, and a log search you had built up filter by filter
 * was gone the moment you looked something up elsewhere. None of them is a
 * property of a route — they are how you have arranged your workspace — so
 * they live above the router and persist until you change them.
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
  const [healthDone, setHealthDone] = useState<HealthDone | null>(null)
  const [healthHideDone, setHealthHideDone] = useState(false)
  const [logFilters, setLogFilters] = useState<LogFilters>(defaultLogFilters)
  const [logRun, setLogRun] = useState<LogRun | null>(null)

  const startLogRun = useCallback((envId: string, query: LogQuery) => {
    setLogRun({ envId, query, older: [] })
  }, [])

  const appendLogPage = useCallback((page: LogPage) => {
    setLogRun((prev) => (prev ? { ...prev, older: [...prev.older, page] } : prev))
  }, [])

  const clearLogSearch = useCallback(() => {
    setLogFilters(defaultLogFilters)
    setLogRun(null)
  }, [])

  const toggleHealthDone = useCallback((reportAt: number, name: string) => {
    setHealthDone((prev) => {
      const names = new Set(prev?.reportAt === reportAt ? prev.names : [])
      if (names.has(name)) names.delete(name)
      else names.add(name)
      return { reportAt, names }
    })
  }, [])

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
      healthDone,
      toggleHealthDone,
      healthHideDone,
      setHealthHideDone,
      logFilters,
      setLogFilters,
      logRun,
      startLogRun,
      appendLogPage,
      clearLogSearch,
    }),
    [
      selected,
      analysisOpen,
      chartsOpen,
      healthMinutes,
      healthFilter,
      healthStatuses,
      healthDone,
      toggleHealthDone,
      healthHideDone,
      logFilters,
      logRun,
      startLogRun,
      appendLogPage,
      clearLogSearch,
    ],
  )

  return <WorkbenchContext.Provider value={value}>{children}</WorkbenchContext.Provider>
}

export function useWorkbench(): WorkbenchValue {
  const context = useContext(WorkbenchContext)
  if (!context) throw new Error('useWorkbench must be used inside a WorkbenchProvider')
  return context
}
