import { formatJson } from "@/lib/format"

export function JsonBlock({ value }: { value: unknown }) {
  return (
    <pre className="overflow-x-auto whitespace-pre-wrap">
      {formatJson(value)}
    </pre>
  )
}
