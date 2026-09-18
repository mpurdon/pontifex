import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { Navigate, RouterProvider, createHashRouter } from 'react-router-dom'
import './index.css'
import { Shell } from './app/shell'
import { SettingsProvider } from './app/settings-context'
import { LoginProvider } from './app/login-dialog'
import { AiDraftProvider } from './features/ai/ai-draft-context'
import { WorkbenchProvider } from './features/schemas/workbench-context'
import { SchemasPage } from './features/schemas/schemas-page'
import { AiPage } from './features/ai/ai-page'
import { LogsPage } from './features/logs/logs-page'
import { WatchPage } from './features/watch/watch-page'
import { ReportPage } from './features/report/report-page'
import { DeveloperPage } from './features/developer/developer-page'
import { TopologyPage } from './features/topology/topology-page'
import { SettingsPage } from './features/settings/settings-page'
import { ErrorBoundary, RouteErrorBoundary } from './app/error-boundary'

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // AWS responses are stable enough that refetching on every window focus
      // would just burn API calls.
      refetchOnWindowFocus: false,
      // Auth failures never succeed on retry; other errors are surfaced with
      // an explicit refresh affordance instead.
      retry: false,
      staleTime: 30_000,
    },
  },
})

// Hash routing: in production the app is served from a custom protocol where
// history-based deep links do not resolve.
/**
 * Each page gets its own `errorElement`, so a crash is contained to the outlet
 * and the nav stays usable — you can walk to another screen rather than
 * reloading the app.
 */
const router = createHashRouter([
  {
    path: '/',
    element: <Shell />,
    errorElement: <RouteErrorBoundary scope="The app shell" />,
    children: [
      { index: true, element: <Navigate to="/schemas" replace /> },
      {
        path: 'schemas',
        element: <SchemasPage />,
        errorElement: <RouteErrorBoundary scope="The Schemas screen" />,
      },
      {
        path: 'generate',
        element: <AiPage />,
        errorElement: <RouteErrorBoundary scope="The Generate screen" />,
      },
      {
        path: 'logs',
        element: <LogsPage />,
        errorElement: <RouteErrorBoundary scope="The Logs screen" />,
      },
      {
        path: 'watch',
        element: <WatchPage />,
        errorElement: <RouteErrorBoundary scope="The Watch screen" />,
      },
      {
        path: 'report',
        element: <ReportPage />,
        errorElement: <RouteErrorBoundary scope="The Health screen" />,
      },
      {
        path: 'topology',
        element: <TopologyPage />,
        errorElement: <RouteErrorBoundary scope="The Topology screen" />,
      },
      {
        path: 'settings',
        element: <SettingsPage />,
        errorElement: <RouteErrorBoundary scope="The Settings screen" />,
      },
      {
        path: 'developer',
        element: <DeveloperPage />,
        errorElement: <RouteErrorBoundary scope="The Developer screen" />,
      },
    ],
  },
])

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    {/*
      Outermost boundary: the providers render above the router, so a crash in
      one of them is out of `errorElement`'s reach and would otherwise blank
      the window with no explanation and nothing in the log.
    */}
    <ErrorBoundary scope="The application">
      <QueryClientProvider client={queryClient}>
        <SettingsProvider>
          <LoginProvider>
            <AiDraftProvider>
              <WorkbenchProvider>
                <RouterProvider router={router} />
              </WorkbenchProvider>
            </AiDraftProvider>
          </LoginProvider>
        </SettingsProvider>
      </QueryClientProvider>
    </ErrorBoundary>
  </StrictMode>,
)
