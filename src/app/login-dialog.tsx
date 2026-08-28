import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { listen } from '@tauri-apps/api/event'
import { openUrl } from '@tauri-apps/plugin-opener'
import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from 'react'
import { Check, Copy, ExternalLink } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type { DeviceAuthorization, Environment, IpcError, LoginResult } from '@/lib/types'
import {
  Button,
  ErrorBox,
  Modal,
  ModalDescription,
  ModalTitle,
  Note,
  Spinner,
} from '@/components/ui'

/**
 * What to sign in to: a named profile, or an SSO session directly.
 *
 * Signing in to the session is the broader action — it covers every account
 * the session grants, including ones no profile exists for.
 */
export type LoginTarget = { profile: string } | { session: string }

function targetLabel(target: LoginTarget): string {
  return 'profile' in target ? target.profile : target.session
}

interface LoginContextValue {
  login: (target: LoginTarget) => void
}

const LoginContext = createContext<LoginContextValue | null>(null)

/**
 * Hosts the SSO sign-in dialog and exposes a `login(profile)` trigger.
 *
 * Sign-in is reachable from anywhere an auth error can surface — the profile
 * list, a failed schema query, the AI panel — so it lives at the app root
 * rather than being duplicated per screen.
 */
export function LoginProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient()
  const [target, setTarget] = useState<LoginTarget | null>(null)
  const [device, setDevice] = useState<DeviceAuthorization | null>(null)
  const [copied, setCopied] = useState(false)

  // The native flow emits the device code before it starts polling, so the
  // user sees the code immediately rather than after the login resolves.
  useEffect(() => {
    const unlisten = listen<DeviceAuthorization>('sso://device-code', (event) => {
      setDevice(event.payload)
    })
    return () => {
      void unlisten.then((fn) => fn())
    }
  }, [])

  const mutation = useMutation<LoginResult, IpcError, { target: LoginTarget; forceCli: boolean }>({
    mutationFn: ({ target: t, forceCli }) =>
      'profile' in t ? ipc.ssoLogin(t.profile, forceCli) : ipc.ssoSessionLogin(t.session),
    onSuccess: () => {
      // Credentials changed; everything that touches AWS is now stale.
      queryClient.invalidateQueries()
    },
  })

  const login = useCallback((next: LoginTarget) => {
    setTarget(next)
    setDevice(null)
    setCopied(false)
    mutation.reset()
  }, [mutation])

  const close = () => {
    setTarget(null)
    setDevice(null)
    mutation.reset()
  }

  const start = (forceCli: boolean) => {
    if (!target) return
    setDevice(null)
    mutation.mutate({ target, forceCli })
  }

  // The CLI fallback shells out to `aws sso login --profile`, which has no
  // session-level equivalent.
  const canUseCli = target !== null && 'profile' in target

  const copyCode = async () => {
    if (!device) return
    await navigator.clipboard.writeText(device.userCode)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }

  return (
    <LoginContext.Provider value={{ login }}>
      {children}

      <Modal open={target !== null} onClose={close} width={440} className="p-4">
            <ModalTitle>Sign in to AWS</ModalTitle>
            <ModalDescription>
              {target && 'profile' in target ? 'Profile' : 'SSO session'}{' '}
              <span className="font-mono text-ink">
                {target ? targetLabel(target) : ''}
              </span>
            </ModalDescription>

            <div className="mt-4 flex flex-col gap-3">
              {mutation.isIdle && (
                <>
                  <Note>
                    Opens your browser to approve the sign-in. The resulting
                    token is written to <span className="font-mono">~/.aws/sso/cache</span>,
                    so the AWS CLI shares the same session.
                    {!canUseCli && (
                      <span className="mt-1 block">
                        This covers every account and role the session grants.
                      </span>
                    )}
                  </Note>
                  <div className="flex gap-2">
                    <Button variant="primary" onClick={() => start(false)}>
                      Sign in
                    </Button>
                    {canUseCli && (
                      <Button variant="ghost" onClick={() => start(true)}>
                        Use AWS CLI instead
                      </Button>
                    )}
                  </div>
                </>
              )}

              {mutation.isPending && !device && <Spinner label="Starting sign-in…" />}

              {mutation.isPending && device && (
                <>
                  <p className="text-xs text-ink-muted">
                    Confirm this code in the browser window that just opened:
                  </p>
                  <div className="flex items-center gap-2">
                    <code className="flex-1 rounded-md border border-edge bg-surface-0 px-3 py-2 text-center font-mono text-lg tracking-[0.2em] text-accent">
                      {device.userCode}
                    </code>
                    <Button variant="secondary" onClick={copyCode} title="Copy code">
                      {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
                    </Button>
                  </div>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() =>
                      void openUrl(device.verificationUriComplete ?? device.verificationUri)
                    }
                  >
                    <ExternalLink className="size-3" />
                    Reopen browser
                  </Button>
                  <Spinner label="Waiting for approval…" />
                </>
              )}

              {mutation.isError && (
                <>
                  <ErrorBox error={mutation.error} />
                  <div className="flex gap-2">
                    <Button variant="secondary" onClick={() => start(false)}>
                      Try again
                    </Button>
                    {canUseCli && (
                      <Button variant="ghost" onClick={() => start(true)}>
                        Use AWS CLI instead
                      </Button>
                    )}
                  </div>
                </>
              )}

              {mutation.isSuccess && (
                <>
                  <div className="rounded-md border border-ok/40 bg-ok/10 px-3 py-2 text-xs text-ok">
                    {mutation.data.message}
                    {mutation.data.expiresAt && (
                      <span className="mt-1 block text-ink-muted">
                        Expires {new Date(mutation.data.expiresAt).toLocaleString()}
                      </span>
                    )}
                    {mutation.data.method === 'cliFallback' && (
                      <span className="mt-1 block text-ink-muted">
                        Built-in sign-in was unavailable, so the AWS CLI was used.
                      </span>
                    )}
                  </div>
                  <Button variant="primary" onClick={close}>
                    Done
                  </Button>
                </>
              )}
            </div>
      </Modal>
    </LoginContext.Provider>
  )
}

export function useLogin(): LoginContextValue {
  const context = useContext(LoginContext)
  if (!context) throw new Error('useLogin must be used inside a LoginProvider')
  return context
}

/**
 * What can be done about an environment's credentials failing.
 *
 * `onLogin` is present only when signing in from here would actually help;
 * `hint` explains what to do instead when it would not.
 */
export interface CredentialAction {
  onLogin?: () => void
  hint?: string
}

/**
 * Convenience for screens that render an `ErrorBox`: signs into whatever the
 * environment's credentials actually come from.
 *
 * - An SSO target signs into its session, which covers every account it grants.
 * - A profile signs into that profile.
 * - `onLogin` is absent when neither can be signed into from here — a static
 *   profile written by an external credential manager (Leapp, aws-vault, …).
 *   `ErrorBox` then explains where to refresh instead of showing a button that
 *   would do nothing.
 */
export function useLoginForEnvironment(
  env: Environment | undefined,
): CredentialAction {
  return useLoginFor(env?.sso?.session, env?.awsProfile)
}

/**
 * Same, for a bare profile name — used by the AI panel, which authenticates as
 * the separately configured Bedrock profile rather than an environment.
 */
export function useLoginForProfileName(
  profile: string | undefined,
): CredentialAction {
  return useLoginFor(undefined, profile)
}

function useLoginFor(
  session: string | undefined,
  profile: string | undefined,
): CredentialAction {
  const { login } = useLogin()

  const { data: profiles } = useQuery({
    queryKey: ['profiles'],
    queryFn: ipc.listProfiles,
    staleTime: 60_000,
    retry: false,
  })

  const callback = useCallback(() => {
    // Signing into the session is preferred: it covers every account it grants.
    if (session) login({ session })
    else if (profile) login({ profile })
  }, [login, session, profile])

  if (session) return { onLogin: callback }
  if (!profile) {
    return { hint: 'This environment has no credentials configured yet.' }
  }

  // Until the profile list loads, assume sign-in is possible: an SSO profile is
  // the common case, and hiding the button then showing it flickers.
  if (!profiles) return { onLogin: callback }

  const match = profiles.find((p) => p.name === profile)

  if (!match) {
    // The profile the environment names has vanished. Tools that write
    // temporary credentials (Leapp, aws-vault) delete the profile when the
    // session ends, so offering a sign-in here would fail with "not found".
    return {
      hint:
        `Profile “${profile}” is no longer in ~/.aws/config. Tools that write ` +
        `temporary credentials remove it when the session ends. Re-activate it ` +
        `there, or switch this environment to an SSO account in Settings.`,
    }
  }

  if (!match.sso.applicable) {
    return {
      hint:
        `Credentials for “${profile}” are managed outside Pontifex. Refresh the ` +
        `session in your credential tool, or switch this environment to an SSO ` +
        `account in Settings.`,
    }
  }

  return { onLogin: callback }
}
