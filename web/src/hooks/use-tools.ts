import { useQuery } from "@tanstack/react-query"

import { api, type ToolInfo } from "@/lib/api"

export function useTools() {
  return useQuery({
    queryKey: ["tools"],
    queryFn: () => api.get<ToolInfo[]>("/api/tools"),
    staleTime: Infinity,
  })
}
