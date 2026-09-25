import { useNavigate } from 'react-router-dom'
import { Braces, FilePlus2 } from 'lucide-react'
import { Button } from '@/components/ui'
import { draftLink, schemaLink } from './schema-links'

/**
 * The open-or-draft pair for one event row.
 *
 * Renders nothing until the registry has answered: offering "draft a schema"
 * for a type that has one is worse than offering nothing for a moment.
 */
export function SchemaRowActions({
  source,
  detailType,
  logGroup,
  timestamp,
  schemaName,
  known,
}: {
  source: string | null
  detailType: string | null
  /** Where this row was read from, so a draft reads the same group. */
  logGroup: string
  /** The event's own time, so a draft samples around it. */
  timestamp: number | null
  schemaName: string | null
  known: boolean
}) {
  const navigate = useNavigate()
  const identity = source && detailType ? `${source}@${detailType}` : null
  if (!identity || !known) return null

  // Clicks stop here: these sit in rows that expand or navigate.
  return schemaName ? (
    <Button
      variant="ghost"
      size="sm"
      title={`Open schema ${schemaName}`}
      onClick={(e) => {
        e.stopPropagation()
        navigate(schemaLink(schemaName))
      }}
    >
      <Braces className="size-3" />
    </Button>
  ) : (
    <Button
      variant="ghost"
      size="sm"
      className="text-warn"
      title={`No schema is registered for ${identity}. Draft one from the events around this one — Health does the wider analysis later.`}
      onClick={(e) => {
        e.stopPropagation()
        navigate(draftLink(identity, logGroup, timestamp ?? Date.now()))
      }}
    >
      <FilePlus2 className="size-3" />
    </Button>
  )
}
