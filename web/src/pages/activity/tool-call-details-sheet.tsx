import type { ReactNode } from "react"

import { JsonBlock } from "@/components/json-block"
import { ToolCallStatusBadge } from "@/components/tool-call-status-badge"
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet"
import type { ToolCall } from "@/lib/api"
import { formatDateTime, formatDuration } from "@/lib/format"

type ToolCallDetailsSheetProps = {
  call: ToolCall | null
  onClose: () => void
}

export function ToolCallDetailsSheet({
  call,
  onClose,
}: ToolCallDetailsSheetProps) {
  return (
    <Sheet open={call !== null} onOpenChange={(open) => !open && onClose()}>
      <SheetContent className="sm:max-w-lg">
        {call && <ToolCallDetails call={call} />}
      </SheetContent>
    </Sheet>
  )
}

function ToolCallDetails({ call }: { call: ToolCall }) {
  return (
    <>
      <SheetHeader>
        <SheetTitle>{call.tool}</SheetTitle>
        <SheetDescription>
          {call.session_name} · {formatDateTime(call.created_at)}
        </SheetDescription>
      </SheetHeader>
      <div className="flex flex-col gap-4 overflow-y-auto px-4 pb-4">
        <div className="flex flex-wrap items-center gap-2">
          <ToolCallStatusBadge status={call.status} />
          {call.duration_ms !== null && formatDuration(call.duration_ms)}
        </div>
        <Section title="Arguments">
          <JsonBlock value={call.arguments} />
        </Section>
        {call.error !== null && (
          <Section title="Error">
            <p className="text-sm whitespace-pre-wrap text-destructive">
              {call.error}
            </p>
          </Section>
        )}
        {call.result !== null && (
          <Section title="Result">
            <JsonBlock value={call.result} />
          </Section>
        )}
      </div>
    </>
  )
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-2">
      <h3 className="font-medium">{title}</h3>
      {children}
    </section>
  )
}
