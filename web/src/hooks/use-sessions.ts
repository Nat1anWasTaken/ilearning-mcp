import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { toast } from "sonner"

import { api, type AgentSession } from "@/lib/api"

export const sessionsQueryKey = ["sessions"] as const

export function useSessions() {
  return useQuery({
    queryKey: sessionsQueryKey,
    queryFn: () => api.get<AgentSession[]>("/api/sessions"),
  })
}

export function useRenameSession(id: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (name: string) =>
      api.patch<AgentSession>(`/api/sessions/${id}`, { name }),
    onSuccess: (updated) => {
      queryClient.setQueryData<AgentSession[]>(sessionsQueryKey, (sessions) =>
        sessions?.map((session) => (session.id === id ? updated : session))
      )
      toast.success("Renamed")
    },
    onError: (error) =>
      toast.error("Couldn't rename", { description: error.message }),
  })
}

export function useDeleteSession(id: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: () => api.delete(`/api/sessions/${id}`),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: sessionsQueryKey }),
  })
}
