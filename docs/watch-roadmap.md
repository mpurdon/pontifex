# Watch mode: what it is for, and what comes next

Watch mode exists so you can find out that something happened on the bus
without sitting there looking for it. Everything below is judged against that
one sentence. A feature that makes the screen prettier while you are staring
at it is not on this list; a feature that gets the right fact to you while you
are doing something else is.

## What is in place (September 2026)

| Need                                            | How it is met                                                                    |
| ----------------------------------------------- | -------------------------------------------------------------------------------- |
| Say when a matching event goes past             | Desktop notification per hit, naming the event and the fields the watch keyed on |
| Do it all day without cost or side effects      | Passive `FilterLogEvents` polling with a cursor; no bus resources; cents a day   |
| Keep going through token expiry, sleep, network | Auth pause with one notification; exponential backoff; capped catch-up on wake   |
| Keep going when the window is closed            | Close hides the window while anything is armed; the Dock icon brings it back     |
| Know whether a watch works or the bus is quiet  | One-hour backfill on arming; per-watch hit count and last-hit age; 24h probe     |
| Know notifications reach this machine           | Test notification button                                                         |
| Come back to what happened                      | Hit log with payloads, unread badge on the nav and Dock, deep link to the schema |

## Next, in order

Each item says what it buys, roughly what it costs, and why it sits where it
does. Effort is in working days for one person who knows the codebase.

### 1. Watch for events that break their schema — 2 days

A watch option: *notify only when the event fails its registered schema*, or
*when it has a field the schema does not declare*. The reality-check code
already validates events against schemas and finds undeclared fields; this
wires it into the poller so a producer shipping a bad payload is a notification
within a minute rather than a Health report you run next week.

This is the feature most specific to Pontifex. Nothing else on the bus can do it
because nothing else has the registry and the traffic in one place.

### 2. Absence alerts — 1 day

The inverse watch: *tell me if `clientProfile-medical` goes quiet for 30
minutes*. The poller already knows when each watch last matched; this adds a
threshold and a notification when it is crossed, and another when traffic
resumes. Dead producers are the failure you never notice from a hit list,
because the hit list simply stops growing.

### 3. Menu-bar presence — 1 day

A tray icon showing armed state and unread count, with a menu to pause every
environment, open the window, or quit. Today the app must stay in the Dock
with a hidden window; a menu-bar item is what "runs all day" looks like on a
Mac, and it makes stopping deliberate rather than accidental.

### 4. Notification throttling and digests — 1 day

Per watch: *at most one notification per N minutes*, with the rest rolled into
a count, and an optional *digest every N minutes* mode for busy sources. The
poller already collapses a burst within one pass into one notification; this
extends that across passes. Without it a watch on a busy source is either
muted or unbearable, and both mean you stop trusting notifications.

### 5. "Watch this" from where you are — 1 day

Buttons that pre-fill a watch: from a Logs row (this source and type), from a
schema (its identity), from a Health report row (the type with no schema), and
from a hit's payload (this `clientId`, anywhere on the bus). Defining a watch
from a blank form means typing names from memory; defining it from the thing
in front of you means no spelling mistakes, and the probe confirms it before
you save.

### 6. Rate alerts — 1 day

*More than N per minute* and *fewer than N per hour*, on any watch. Spikes and
slowdowns are the other two things a hit list cannot show you. Shares the
per-watch counters absence alerts introduce.

### 7. Follow an id across sources — 1 day

From a hit, one click watches the same `clientId` (or any id-like field) on
every source, and the hit list can group by that id. This turns a watch into a
trace: you see the whole path an entity took through the bus, in order, as it
happens.

### 8. Post hits somewhere other than this laptop — 2 days

A per-watch webhook or Slack channel, so a hit reaches the team and not only
the person who happens to have Pontifex open. This is the point at which watch
mode stops being a personal tool. It needs care around what payload leaves the
machine, and it should default to sending the identity of the event rather
than the whole payload.

### 9. Cost and coverage meter — half a day

Show what the current watches cost per day (bytes scanned, calls made) and
how far behind the poller is. Mostly reassurance, but it is the number that
lets someone leave it running on prd without wondering.

### 10. On-demand live tail — 1 day

When you *are* sitting and watching, `StartLiveTail` gives sub-second delivery
for one session. Deliberately separate from watch mode: it is a streaming
session that costs by the minute and times out, which is the wrong shape for
all-day use and the right shape for a five-minute debugging session.

## Not planned

- **EventBridge rules or queues as the transport.** Faster, but it changes the
  bus and shows up in every topology diff. The passive design is the point.
- **Server-side alerting (CloudWatch alarms, metric filters).** Real
  monitoring belongs in the monitoring stack. Watch mode is for the person at
  the keyboard who wants to know about one thing today.
- **A general log search.** The Logs screen does that. Watch mode is about
  what happens next, not what happened before.
