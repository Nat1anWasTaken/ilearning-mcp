import { useMutation } from "@tanstack/react-query"

import { api, type LinkStart } from "@/lib/api"

/** `POST /auth/ilearning/start`: a new link attempt (works signed in or out). */
export function useStartLink() {
  return useMutation({
    mutationFn: (next: string) =>
      api.post<LinkStart>("/auth/ilearning/start", { next }),
  })
}

/** `POST /auth/ilearning/complete`; navigates away on success. */
export function useCompleteLink() {
  return useMutation({
    mutationFn: (body: { attempt_id: string; pasted: string }) =>
      api.post<{ redirect_to: string }>("/auth/ilearning/complete", body),
    // A full navigation: `redirect_to` may continue an /oauth/authorize flow.
    onSuccess: ({ redirect_to }) => window.location.assign(redirect_to),
  })
}
