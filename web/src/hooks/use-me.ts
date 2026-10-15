import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useNavigate } from "react-router"

import { api, type Me } from "@/lib/api"

export const meQueryKey = ["me"] as const

export function useMe() {
  return useQuery({
    queryKey: meQueryKey,
    queryFn: () => api.get<Me>("/api/me"),
  })
}

export function useLogout() {
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  return useMutation({
    mutationFn: () => api.post<void>("/auth/logout"),
    onSuccess: () => {
      queryClient.clear()
      void navigate("/login")
    },
  })
}

/** `/api/me` for components rendered inside `<RequireAuth>`. */
export function useSignedInMe(): Me {
  const { data } = useMe()
  if (!data) {
    throw new Error("useSignedInMe must be used inside <RequireAuth>")
  }
  return data
}
