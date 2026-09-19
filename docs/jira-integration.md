# Jira integration

File a ticket for a producer bug from the place the bug is visible, routed to
the team that owns the event source.

The analysis panel already knows everything a ticket needs — what disagrees,
how much traffic it affects, whether events are being rejected, and an
offending payload. What it lacked was a way to hand that to the team that can
fix it without retyping any of it.

## Decisions

| Question | Answer |
| --- | --- |
| Deployment | Jira Cloud (`*.atlassian.net`) |
| Auth | OAuth 2.0 (3LO) in the system browser; the work IdP handles SSO |
| Team model | One Jira project per pod — routing maps an event `source` to a project key, or the owning team or repository when no source rule says |
| Entry points | Per issue in the Analysis panel; bulk from the Health report, including types with no schema; per hit in Watch |
| REST version | v2, not v3 |

### Why v2

v2 takes a plain-string `description` (wiki markup, `{code}` blocks). v3
requires assembling Atlassian Document Format JSON for the same output. v2 is
current for Cloud and is what Atlassian's own OAuth examples use.

### The client secret

Atlassian 3LO supports only the authorization code grant — no implicit flow,
no public-client PKCE — so the token exchange needs a `client_secret`. It
therefore cannot be compiled into the binary or committed. It is entered once
in Settings and stored in the OS keychain with the tokens.

This means the integration is inert until someone registers an app in the
Atlassian developer console and provides a client ID and secret. That is the
only external dependency.

## Setting it up

The console generates the whole authorization URL once an app is configured,
and it already contains the client ID, the callback as registered and the
scopes actually granted. Settings → Jira takes that URL and reads the values
out of it, rather than asking for each to be transcribed into a field where
one wrong character fails at consent time with a message about none of this.
It also names any scope pontifex needs that the app does not have.

The callback URL is stored whole and sent verbatim, because `localhost` and
`127.0.0.1` are the same machine but not the same string, and Atlassian
compares strings. It must be loopback — a redirect anywhere else would consent
successfully and hand the code to a machine that is not this one. `localhost`
binds both `127.0.0.1` and `::1`, since browsers pick either.

## Flow

1. **Authorize** — open
   `https://auth.atlassian.com/authorize?audience=api.atlassian.com&client_id=…&scope=…&redirect_uri=…&state=…&response_type=code&prompt=consent`
   in the system browser. A loopback listener on a fixed port catches the
   redirect, checks `state`, and answers with a page telling the user to close
   the tab.
2. **Exchange** — `POST https://auth.atlassian.com/oauth/token` with the code
   and the secret. Access tokens last about an hour; `offline_access` is what
   makes a refresh token possible, and refresh tokens rotate, so the new one
   must be persisted on every refresh.
3. **Resolve the site** — `GET https://api.atlassian.com/oauth/token/accessible-resources`
   returns the sites this grant covers. Its `id` is the `cloudid`.
4. **Call the API** — `https://api.atlassian.com/ex/jira/{cloudid}/rest/api/2/…`
   with the access token as a bearer token.

Scopes: `read:jira-work write:jira-work read:jira-user offline_access`.

## Phases

1. **Connect.** `src-tauri/src/jira/` — `oauth.rs` (browser + loopback +
   exchange), `tokens.rs` (keychain), `client.rs` (cloudid, refresh, request).
   Settings gains a Jira block; Settings UI gains connect/disconnect and a site
   picker.
2. **File one ticket.** An `Issue` becomes a ticket: `summary` → title, and
   `action`, `declared`/`observed`, counts, example → body. A preview dialog
   shows the exact ticket before anything is written. On success the issue row
   links to the created ticket.
3. **Routing.** A settings table of `source pattern → project key` (plus issue
   type, labels, optional assignee), with a fallback project. Validated against
   `/project` and per-project `createmeta`, so a wrong key or a missing issue
   type is caught in Settings rather than at 403 time.
4. **Dedupe.** Every ticket carries a `pontifex-<fingerprint>` label derived from
   schema + environment + `issue.key`. Before creating, search for an open
   ticket with that label and offer to comment instead. This is what stops
   phase 5 being a spam cannon.
5. **Bulk.** Select failing schemas in the Health report, group by source,
   preview exactly what will be created and what was skipped as already filed,
   then create in one batch with per-row results.

## Every discrepancy is fileable

A ticket can be filed from every place Pontifex shows the bus disagreeing with
the registry, not only from a graded schema:

- **A schema that is wrong or drifting** — per issue in the Analysis panel,
  and in bulk from the Health report.
- **A type with no schema at all** — the Health report's *on the bus with no
  schema* rows select for bulk filing like any other. The issue kind is
  `unregistered`: one row per type, saying that the source publishes it and
  the registry has nothing for it, with an example payload. Bulk filing
  pre-selects these along with the rejected ones — the absence is the whole
  finding.
- **Something a person sees that the validator cannot** — a misleading
  name, a field that should not exist, a type that fits the traffic but is
  the wrong contract. The bug icon beside the side-panel tabs raises a
  concern about the event type; the one beside a field's name in Details
  raises it about that field. Two sentences are asked for, the title and the
  detail, and the ticket goes through the same preview with the schema's
  declaration and the field's presence in sampled events as evidence. The
  issue kind is `concern`, keyed by path, so a second concern about the same
  field is offered as a comment on the open ticket.
- **A single event caught by a watch** — *Check against schema* on an
  expanded hit validates that one payload against the registered schema and
  lists what disagrees, or says there is no schema. Each problem files with
  the hit's environment and log group as context.

## Routing by owner

A source rule needs someone to have written it. The origin lookup already
knows who publishes a type — the CODEOWNERS team and the repository of the
file that puts it on the bus — so a second table in Settings → Jira routes by
those: `@acme/payments → PAY`, `acme/billing-* → BILL`. It is consulted only
when no source rule matches, and only for types whose origin has been looked
up; the cache is read, never GitHub, so a preview costs nothing extra. The
ticket body gains a *Publisher* section naming the file, the owners, and who
first published the type in which pull request. Filing falls through to the
default project as before when neither table matches.

## Fields a project demands

Projects are free to make any field mandatory — a "Discovery Environment", a
team, a component — and a create call that omits one is rejected with a message
naming a custom field id and nothing else. pontifex asks Jira what the target
project requires (`createmeta`), shows those fields in the preview, and stores
the answers per project so the question is asked once rather than per ticket.

Per project rather than per routing rule: the requirement belongs to the
project, and several rules can point at one. Values are stored in the shape
Jira wants — `{"id": "10500"}`, an array of those, a bare string, a number —
because that is what varies between field types.

Bulk filing uses those stored answers. A project nobody has filed into yet may
therefore fail its first bulk run; filing one ticket from the Analysis panel
answers the question and unblocks the rest.

## Rules

- Nothing is ever filed silently. Every write goes through a preview the user
  confirms, the same way saving to a protected environment does.
- The secret and tokens live in the OS keychain, never in `settings.json`.
- A 403 from a target project reads as "you cannot create issues in PROJ", not
  as a bare status code.

## Risks

- The loopback port is fixed so it can match the registered callback exactly;
  a port collision needs a clear error rather than a hang.
- The consent screen makes the user pick a site, so an account-level grant and
  a site-restricted one return different `accessible-resources` sets.
- Issue type names vary per project — `createmeta` decides, not a hardcoded
  "Bug".
