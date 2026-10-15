import { useMutation, useQuery } from "@tanstack/react-query"

import { api, type OAuthRequest } from "@/lib/api"

type OAuthDecision = {
  approve: boolean
  name?: string
}

export function useOAuthRequest(id: string) {
  return useQuery({
    queryKey: ["oauth-request", id],
    queryFn: () => api.get<OAuthRequest>(`/api/oauth/requests/${id}`),
    // Never refetch: once decided the request is gone (404), and focus often
    // returns here after the client's redirect.
    staleTime: Infinity,
  })
}

export function useDecideOAuthRequest(id: string) {
  return useMutation({
    mutationFn: (decision: OAuthDecision) =>
      api.post<{ redirect_to: string }>(`/api/oauth/requests/${id}`, decision),
    onSuccess: ({ redirect_to }) => {
      window.location.assign(redirect_to)
    },
  })
}
