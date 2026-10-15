import { useState } from "react"
import { ActivityIcon } from "lucide-react"
import { useSearchParams } from "react-router"

import { ErrorAlert } from "@/components/error-alert"
import { PageHeader } from "@/components/page-header"
import { RelativeTime } from "@/components/relative-time"
import {
  TOOL_CALL_STATUSES,
  ToolCallStatusBadge,
} from "@/components/tool-call-status-badge"
import { Button } from "@/components/ui/button"
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty"
import { Skeleton } from "@/components/ui/skeleton"
import { Spinner } from "@/components/ui/spinner"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"
import { useToolCallLog, type ToolCallFilters } from "@/hooks/use-tool-calls"
import type { ToolCallStatus } from "@/lib/api"
import { formatDuration } from "@/lib/format"
import { ActivityFilters } from "@/pages/activity/activity-filters"
import { ToolCallDetailsSheet } from "@/pages/activity/tool-call-details-sheet"

export function ActivityPage() {
  const [searchParams, setSearchParams] = useSearchParams()
  const filters: ToolCallFilters = {
    session_id: searchParams.get("session_id") ?? undefined,
    tool: searchParams.get("tool") ?? undefined,
    status: parseStatus(searchParams.get("status")),
  }

  function setFilter(key: keyof ToolCallFilters, value: string | undefined) {
    setSearchParams((params) => {
      if (value) {
        params.set(key, value)
      } else {
        params.delete(key)
      }
      return params
    })
  }

  return (
    <>
      <PageHeader
        title="Activity"
        description="Every tool call your AI clients made with your iLearning account."
      />
      <ActivityFilters filters={filters} onFilterChange={setFilter} />
      <ToolCallLog filters={filters} />
    </>
  )
}

function parseStatus(value: string | null): ToolCallStatus | undefined {
  return value !== null && Object.hasOwn(TOOL_CALL_STATUSES, value)
    ? (value as ToolCallStatus)
    : undefined
}

function ToolCallLog({ filters }: { filters: ToolCallFilters }) {
  const log = useToolCallLog(filters)
  const [selectedId, setSelectedId] = useState<string | null>(null)

  if (log.isPending) {
    return <Skeleton className="h-64 w-full" />
  }
  if (log.isError) {
    return <ErrorAlert title="Couldn't load activity" error={log.error} />
  }

  const calls = log.data.pages.flatMap((page) => page.items)
  // Read from the (polled) query data so the sheet shows the latest status.
  const selected = calls.find((call) => call.id === selectedId) ?? null
  if (calls.length === 0) {
    return (
      <Empty>
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <ActivityIcon />
          </EmptyMedia>
          <EmptyTitle>No tool calls</EmptyTitle>
          <EmptyDescription>
            Tool calls show up here as soon as an agent makes them.
          </EmptyDescription>
        </EmptyHeader>
      </Empty>
    )
  }

  return (
    <div className="flex flex-col gap-4">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Time</TableHead>
            <TableHead>Client</TableHead>
            <TableHead>Tool</TableHead>
            <TableHead>Status</TableHead>
            <TableHead>Duration</TableHead>
            <TableHead>
              <span className="sr-only">Details</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {calls.map((call) => (
            <TableRow key={call.id}>
              <TableCell>
                <RelativeTime value={call.created_at} />
              </TableCell>
              <TableCell>{call.session_name}</TableCell>
              <TableCell>{call.tool}</TableCell>
              <TableCell>
                <ToolCallStatusBadge status={call.status} />
              </TableCell>
              <TableCell>
                {call.duration_ms === null
                  ? "—"
                  : formatDuration(call.duration_ms)}
              </TableCell>
              <TableCell>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => setSelectedId(call.id)}
                >
                  Details
                </Button>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      {log.hasNextPage && (
        <Button
          variant="outline"
          className="self-center"
          disabled={log.isFetchingNextPage}
          onClick={() => void log.fetchNextPage()}
        >
          {log.isFetchingNextPage && <Spinner />}
          Load more
        </Button>
      )}
      <ToolCallDetailsSheet
        call={selected}
        onClose={() => setSelectedId(null)}
      />
    </div>
  )
}
