import type { Environment, Settings } from '@/lib/types'

/**
 * What an AWS account is used for in gebman.
 *
 * There are exactly two jobs, and they are unrelated: reading and writing the
 * EventBridge schema registry and its log groups, and calling Bedrock to
 * generate schemas. They are configured in different places and usually point
 * at different accounts, so a flat list of profiles gives no way to tell which
 * failure matters for what you are doing.
 */
export interface CredentialRole {
  /** Environment labels this credential backs, e.g. `['dev', 'prd']`. */
  eventBus: string[]
  /** True when this credential is the one Bedrock calls go through. */
  bedrock: boolean
}

export const NO_ROLE: CredentialRole = { eventBus: [], bedrock: false }

export function hasRole(role: CredentialRole): boolean {
  return role.eventBus.length > 0 || role.bedrock
}

/**
 * The credential an environment actually authenticates with.
 *
 * An in-app SSO target takes precedence over the named profile, and when one
 * is set the profile field is ignored entirely — so attributing an environment
 * to a profile it no longer uses would be actively misleading.
 */
export function environmentCredentialLabel(env: Environment): string {
  return env.sso ? `${env.sso.accountId}/${env.sso.roleName}` : env.awsProfile
}

/** Whether an environment is backed by an in-app SSO target rather than a profile. */
export function usesSsoTarget(env: Environment): boolean {
  return !!env.sso
}

/**
 * Map every configured profile name to the jobs it does.
 *
 * Keyed by profile name; environments backed by an SSO target contribute
 * nothing here because no profile is involved in resolving them.
 */
export function profileRoles(settings: Settings | null): Map<string, CredentialRole> {
  const roles = new Map<string, CredentialRole>()
  if (!settings) return roles

  const entry = (name: string): CredentialRole => {
    const existing = roles.get(name)
    if (existing) return existing
    const created: CredentialRole = { eventBus: [], bedrock: false }
    roles.set(name, created)
    return created
  }

  for (const env of settings.environments) {
    if (env.sso) continue
    if (!env.awsProfile) continue
    entry(env.awsProfile).eventBus.push(env.label)
  }

  if (settings.llm.awsProfile) {
    entry(settings.llm.awsProfile).bedrock = true
  }

  return roles
}

/** Environments backed by an in-app SSO target rather than a named profile. */
export function ssoBackedEnvironments(settings: Settings | null): Environment[] {
  return (settings?.environments ?? []).filter((e) => e.sso)
}
