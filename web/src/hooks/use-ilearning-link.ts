import { useMutation, useQueryClient } from "@tanstack/react-query"
import { api, meQueryKey } from "@nat1an/mcp-backbone-dashboard"

import type { LinkStart } from "@/lib/types"

/** `POST /auth/ilearning/start`: a new link attempt (works signed in or out). */
export function useStartLink() {
  return useMutation({
    mutationFn: (next: string) =>
      api.post<LinkStart>("/auth/ilearning/start", { next }),
  })
}

/** `POST /auth/ilearning/complete`; navigates away on success. */
export function useCompleteLink() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: { attempt_id: string; pasted: string }) =>
      api.post<{ redirect_to: string }>("/auth/ilearning/complete", body),
    onSuccess: async ({ redirect_to }) => {
      // The user and link just changed.
      await queryClient.invalidateQueries({ queryKey: meQueryKey })
      // A full navigation: `redirect_to` may continue an /oauth/authorize flow.
      window.location.assign(redirect_to)
    },
  })
}
