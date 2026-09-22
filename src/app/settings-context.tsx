import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { createContext, useContext, useMemo, type ReactNode } from 'react'
import * as ipc from '@/lib/ipc'
import type { Environment, Settings, TimeZone } from '@/lib/types'
import { DEFAULT_ZOOM } from './zoom'

interface SettingsContextValue {
  settings: Settings | undefined
  isLoading: boolean
  activeEnvironment: Environment | undefined
  /** The id every data query keys on and every command is scoped to. */
  envId: string | undefined
  setActiveEnvironment: (envId: string) => void
  saveSettings: (settings: Settings) => Promise<Settings>
  /**
   * Persist one panel group's sizes.
   *
   * Separate from `saveSettings` because pane geometry must not invalidate
   * credentials or refetch AWS data; it still refreshes the cached settings so
   * a later full save does not write back stale sizes.
   */
  savePanelSizes: (id: string, sizes: number[]) => void
  /** The zone times are shown in; `local` until settings load. */
  timeZone: TimeZone
  /** Same light path as panel sizes: a display choice must not refetch AWS. */
  setTimeZone: (zone: TimeZone) => void
  /** A theme id or `system`; see `src/theme/themes.ts`. Same light path. */
  setTheme: (theme: string) => void
  /** Webview zoom factor; `1` until settings load. */
  zoom: number
  /** Persists the level only — applying it is `applyZoom` in `app/zoom.ts`. */
  setZoom: (zoom: number) => void
  isSaving: boolean
}

const SettingsContext = createContext<SettingsContextValue | null>(null)

export function SettingsProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient()

  const { data: settings, isLoading } = useQuery({
    queryKey: ['settings'],
    queryFn: ipc.getSettings,
    // Settings only change through this app, so there is nothing to poll for.
    staleTime: Infinity,
  })

  const setActive = useMutation({
    mutationFn: ipc.setActiveEnvironment,
    onSuccess: (next) => {
      queryClient.setQueryData(['settings'], next)
      // Everything downstream is scoped to the environment.
      queryClient.invalidateQueries({ queryKey: ['schemas'] })
      queryClient.invalidateQueries({ queryKey: ['logs'] })
      queryClient.invalidateQueries({ queryKey: ['topology'] })
    },
  })

  const save = useMutation({
    mutationFn: ipc.saveSettings,
    onSuccess: (next) => {
      queryClient.setQueryData(['settings'], next)
      queryClient.invalidateQueries({ queryKey: ['profiles'] })
      queryClient.invalidateQueries({ queryKey: ['schemas'] })
      queryClient.invalidateQueries({ queryKey: ['logs'] })
      queryClient.invalidateQueries({ queryKey: ['topology'] })
    },
  })

  // The light path: display preferences replace the cached settings and
  // invalidate nothing, because nothing downstream depends on them.
  const applyLight = (next: Settings) => queryClient.setQueryData(['settings'], next)

  const savePanels = useMutation({
    mutationFn: ({ id, sizes }: { id: string; sizes: number[] }) =>
      ipc.savePanelSizes(id, sizes),
    onSuccess: applyLight,
  })
  const setZone = useMutation({ mutationFn: ipc.setTimeZone, onSuccess: applyLight })
  const setTheme = useMutation({ mutationFn: ipc.setTheme, onSuccess: applyLight })
  const setZoom = useMutation({ mutationFn: ipc.setZoom, onSuccess: applyLight })

  // Depend on the stable `mutate` functions, not the mutation objects: those
  // are new every render, and a value that changed with them would re-render
  // every consumer (the editors included) on each pane drag.
  const { mutate: setActiveMutate } = setActive
  const { mutateAsync: saveMutateAsync, isPending: isSaving } = save
  const { mutate: savePanelsMutate } = savePanels
  const { mutate: setZoneMutate } = setZone
  const { mutate: setThemeMutate } = setTheme
  const { mutate: setZoomMutate } = setZoom

  const value = useMemo<SettingsContextValue>(() => {
    const activeEnvironment = settings?.environments.find(
      (e) => e.id === settings.activeEnvironmentId,
    )
    return {
      settings,
      isLoading,
      activeEnvironment,
      envId: activeEnvironment?.id,
      setActiveEnvironment: setActiveMutate,
      saveSettings: saveMutateAsync,
      savePanelSizes: (id, sizes) => savePanelsMutate({ id, sizes }),
      timeZone: settings?.timeZone ?? 'local',
      setTimeZone: setZoneMutate,
      setTheme: setThemeMutate,
      zoom: settings?.zoom ?? DEFAULT_ZOOM,
      setZoom: setZoomMutate,
      isSaving,
    }
  }, [
    settings,
    isLoading,
    setActiveMutate,
    saveMutateAsync,
    isSaving,
    savePanelsMutate,
    setZoneMutate,
    setThemeMutate,
    setZoomMutate,
  ])

  return (
    <SettingsContext.Provider value={value}>{children}</SettingsContext.Provider>
  )
}

export function useSettings(): SettingsContextValue {
  const context = useContext(SettingsContext)
  if (!context) {
    throw new Error('useSettings must be used inside a SettingsProvider')
  }
  return context
}
