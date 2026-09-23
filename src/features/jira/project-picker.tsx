import type { JiraProject } from '@/lib/types'
import { Input, SelectMenu } from '@/components/ui'

/**
 * A project chosen from Jira, or typed when Jira cannot be reached.
 *
 * Both the per-rule and the default-project fields in Settings need the same
 * three behaviours, and so does the ticket dialog, where the routed project is
 * a suggestion rather than a decision: pick from the real list, keep showing a
 * key that is set but not visible, and degrade to free text when the list
 * could not be read.
 */
export function ProjectPicker({
  value,
  projects,
  emptyLabel,
  onChange,
}: {
  value: string
  /** Undefined until Jira answers — or forever, if it cannot. */
  projects: JiraProject[] | undefined
  emptyLabel: string
  onChange: (key: string) => void
}) {
  if (!projects) {
    return (
      <Input
        value={value}
        onChange={(e) => onChange(e.target.value.toUpperCase())}
        placeholder="IPP"
        className="font-mono"
        spellCheck={false}
      />
    )
  }

  return (
    <SelectMenu
      value={value}
      onChange={onChange}
      placeholder={emptyLabel}
      className="font-mono"
      options={[
        /* A key set before the list loaded, or from a project you can no
           longer see, still shows rather than silently vanishing. */
        ...(value && !projects.some((p) => p.key === value)
          ? [{ value, label: `${value} — not visible to you` }]
          : []),
        ...projects.map((project) => ({
          value: project.key,
          label: `${project.key} — ${project.name}`,
        })),
      ]}
    />
  )
}

/** An issue type this project offers, or free text until it says. */
export function IssueTypePicker({
  value,
  types,
  fallbackLabel,
  onChange,
}: {
  value: string
  types: string[] | undefined
  /** Shown for the empty choice, or for a value the project does not offer. */
  fallbackLabel: string
  onChange: (type: string) => void
}) {
  if (!types) {
    return (
      <Input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={fallbackLabel}
        spellCheck={false}
      />
    )
  }

  return (
    <SelectMenu
      value={value}
      onChange={onChange}
      placeholder={fallbackLabel}
      options={[
        ...(value && !types.includes(value) ? [{ value, label: value }] : []),
        ...types.map((type) => ({ value: type, label: type })),
      ]}
    />
  )
}
