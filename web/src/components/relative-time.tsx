import { formatDateTime, formatRelativeTime } from "@/lib/format"

/** "5 minutes ago", with the exact time on hover. */
export function RelativeTime({ value }: { value: string }) {
  return (
    <time dateTime={value} title={formatDateTime(value)}>
      {formatRelativeTime(value)}
    </time>
  )
}
