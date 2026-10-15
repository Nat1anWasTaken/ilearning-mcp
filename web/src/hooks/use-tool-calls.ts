import { useInfiniteQuery } from "@tanstack/react-query"

import {
  api,
  withQuery,
  type ToolCallPage,
  type ToolCallStatus,
} from "@/lib/api"

export type ToolCallFilters = {
  session_id?: string
  tool?: string
  status?: ToolCallStatus
}

const toolCallsQueryKey = ["tool-calls"] as const

function fetchToolCalls(
  params: ToolCallFilters & { before?: string; limit?: number }
) {
  return api.get<ToolCallPage>(withQuery("/api/tool-calls", params))
}

export function useToolCallLog(filters: ToolCallFilters) {
  return useInfiniteQuery({
    queryKey: [...toolCallsQueryKey, "log", filters],
    queryFn: ({ pageParam }) =>
      fetchToolCalls({ ...filters, before: pageParam, limit: 50 }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (lastPage) => lastPage.next_before ?? undefined,
    // Keep polling while any loaded call is still running.
    refetchInterval: (query) =>
      query.state.data?.pages.some((page) =>
        page.items.some((call) => call.status === "running")
      )
        ? 3000
        : false,
  })
}
