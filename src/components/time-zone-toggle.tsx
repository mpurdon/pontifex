import { Segmented } from '@/components/ui'
import { useSettings } from '@/app/settings-context'
import type { TimeZone } from '@/lib/types'

const OPTIONS: readonly { id: TimeZone; label: string; title: string }[] = [
  { id: 'local', label: 'local', title: 'Show times in this machine\u2019s zone' },
  {
    id: 'utc',
    label: 'UTC',
    title: 'Show times in UTC \u2014 what CloudWatch, EventBridge\u2019s `time` and the AWS console use',
  },
]

/**
 * Local or UTC, for every clock on the screen.
 *
 * Sits in the toolbar of each screen that lists events, because that is where
 * you are when you want to line a row up against the AWS console. One setting,
 * remembered across restarts, so switching it here switches it everywhere.
 */
export function TimeZoneToggle() {
  const { timeZone, setTimeZone } = useSettings()
  return <Segmented size="sm" value={timeZone} options={OPTIONS} onChange={setTimeZone} />
}
