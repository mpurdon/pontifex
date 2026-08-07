import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { createContext, useContext, useMemo, type ReactNode } from 'react'
import * as ipc from '@/lib/ipc'
import type { Environment, Settings } from '@/lib/types'

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

  const savePanels = useMutation({
    mutationFn: ({ id, sizes }: { id: string; sizes: number[] }) =>
      ipc.savePanelSizes(id, sizes),
    // No query invalidation: nothing downstream depends on pane widths.
    onSuccess: (next) => queryClient.setQueryData(['settings'], next),
  })

  const value = useMemo<SettingsContextValue>(() => {
    const activeEnvironment = settings?.environments.find(
      (e) => e.id === settings.activeEnvironmentId,
    )
    return {
      settings,
      isLoading,
      activeEnvironment,
      envId: activeEnvironment?.id,
      setActiveEnvironment: (envId) => setActive.mutate(envId),
      saveSettings: (next) => save.mutateAsync(next),
      savePanelSizes: (id, sizes) => savePanels.mutate({ id, sizes }),
      isSaving: save.isPending,
    }
  }, [settings, isLoading, setActive, save, savePanels])

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
