const dateTimeFormat = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
})

const relativeTimeFormat = new Intl.RelativeTimeFormat(undefined, {
  numeric: "auto",
})

const RELATIVE_UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 365 * 24 * 60 * 60],
  ["month", 30 * 24 * 60 * 60],
  ["day", 24 * 60 * 60],
  ["hour", 60 * 60],
  ["minute", 60],
  ["second", 1],
]

export function formatDateTime(iso: string): string {
  return dateTimeFormat.format(new Date(iso))
}

/** "3 minutes ago", "yesterday", … */
export function formatRelativeTime(iso: string): string {
  const seconds = (new Date(iso).getTime() - Date.now()) / 1000
  const [unit, unitSeconds] = RELATIVE_UNITS.find(
    ([, unitSeconds]) => Math.abs(seconds) >= unitSeconds
  ) ?? ["second", 1]
  return relativeTimeFormat.format(Math.round(seconds / unitSeconds), unit)
}

export function formatDuration(ms: number): string {
  return ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`
}

/** "4:05" for a span of milliseconds, clamped at zero. */
export function formatCountdown(ms: number): string {
  const totalSeconds = Math.max(0, Math.ceil(ms / 1000))
  const minutes = Math.floor(totalSeconds / 60)
  const seconds = totalSeconds % 60
  return `${minutes}:${seconds.toString().padStart(2, "0")}`
}

export function formatJson(value: unknown): string {
  return JSON.stringify(value, null, 2)
}
