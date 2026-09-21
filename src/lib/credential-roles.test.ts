import { describe, expect, it } from 'vitest'
import type { Environment, Settings } from '@/lib/types'
import {
  environmentCredentialLabel,
  hasRole,
  profileRoles,
  ssoBackedEnvironments,
} from './credential-roles'

function env(overrides: Partial<Environment>): Environment {
  return {
    id: overrides.label ?? 'dev',
    label: 'dev',
    awsProfile: '',
    region: 'us-east-2',
    registryName: 'dev-global-registry',
    logGroups: ['/aws/events/dev-global-events'],
    protected: false,
    ...overrides,
  }
}

function settings(overrides: Partial<Settings>): Settings {
  return {
    environments: [],
    activeEnvironmentId: null,
    llm: {
      awsProfile: null,
      region: 'us-east-2',
      models: [],
      selectedModelId: null,
      claudeSettingsPath: null,
    },
    scan: { maxSeconds: 30, maxEvents: 5000, cacheMb: 32 },
    github: { org: null, ignore: [] },
    jira: {
      clientId: '',
      callbackUrl: 'http://127.0.0.1:53682/callback',
      defaultProject: null,
      issueType: 'Bug',
      labels: [],
      routes: [],
      fieldDefaults: {},
    },
    eventBusRepoPath: null,
    pinnedSources: [],
    panelSizes: {},
    developerMode: false,
    logLevel: 'info',
    timeZone: 'local',
    ...overrides,
  }
}

describe('profileRoles', () => {
  it('tags a profile with every environment it backs', () => {
    const roles = profileRoles(
      settings({
        environments: [
          env({ label: 'dev', awsProfile: 'global-event-bus' }),
          env({ label: 'prd', awsProfile: 'global-event-bus' }),
        ],
      }),
    )
    expect(roles.get('global-event-bus')?.eventBus).toEqual(['dev', 'prd'])
    expect(roles.get('global-event-bus')?.bedrock).toBe(false)
  })

  it('tags the Bedrock profile separately from event bus profiles', () => {
    const roles = profileRoles(
      settings({
        environments: [env({ label: 'dev', awsProfile: 'global-event-bus' })],
        llm: {
          awsProfile: 'claude-code-bedrock',
          region: 'us-east-2',
          models: [],
          selectedModelId: null,
          claudeSettingsPath: null,
        },
      }),
    )
    expect(roles.get('claude-code-bedrock')).toEqual({ eventBus: [], bedrock: true })
    expect(roles.get('global-event-bus')).toEqual({
      eventBus: ['dev'],
      bedrock: false,
    })
  })

  it('lets one profile serve both jobs', () => {
    const roles = profileRoles(
      settings({
        environments: [env({ label: 'dev', awsProfile: 'shared' })],
        llm: {
          awsProfile: 'shared',
          region: 'us-east-2',
          models: [],
          selectedModelId: null,
          claudeSettingsPath: null,
        },
      }),
    )
    expect(roles.get('shared')).toEqual({ eventBus: ['dev'], bedrock: true })
  })

  it('does not credit a profile an SSO-backed environment ignores', () => {
    // `sso` takes precedence over `awsProfile`, so tagging the stale profile
    // name would point the user at a credential that is never resolved.
    const roles = profileRoles(
      settings({
        environments: [
          env({
            label: 'prd',
            awsProfile: 'leftover-profile',
            sso: { session: 'trajector', accountId: '111', roleName: 'ReadOnly' },
          }),
        ],
      }),
    )
    expect(roles.has('leftover-profile')).toBe(false)
  })

  it('reports SSO-backed environments so they can be explained', () => {
    const backed = ssoBackedEnvironments(
      settings({
        environments: [
          env({ label: 'dev', awsProfile: 'p' }),
          env({
            label: 'prd',
            sso: { session: 'trajector', accountId: '111', roleName: 'ReadOnly' },
          }),
        ],
      }),
    )
    expect(backed.map((e) => e.label)).toEqual(['prd'])
  })

  it('is empty before settings have loaded', () => {
    expect(profileRoles(null).size).toBe(0)
    expect(ssoBackedEnvironments(null)).toEqual([])
  })
})

describe('hasRole', () => {
  it('is false only when a profile does neither job', () => {
    expect(hasRole({ eventBus: [], bedrock: false })).toBe(false)
    expect(hasRole({ eventBus: ['dev'], bedrock: false })).toBe(true)
    expect(hasRole({ eventBus: [], bedrock: true })).toBe(true)
  })
})

describe('environmentCredentialLabel', () => {
  it('names the SSO account and role when one is set', () => {
    expect(
      environmentCredentialLabel(
        env({ awsProfile: 'ignored', sso: { session: 's', accountId: '123', roleName: 'Admin' } }),
      ),
    ).toBe('123/Admin')
  })

  it('falls back to the profile name', () => {
    expect(environmentCredentialLabel(env({ awsProfile: 'global-event-bus' }))).toBe(
      'global-event-bus',
    )
  })
})
