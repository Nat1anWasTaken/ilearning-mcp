import { MutationCache, QueryCache, QueryClient } from "@tanstack/react-query"

import { ApiError, isUnauthorized } from "@/lib/api"
import { meQueryKey } from "@/hooks/use-me"

// A 401 anywhere means the dashboard session ended: refetch `/api/me` so the
// auth guard sends the user to the login page.
function handleUnauthorized(error: Error) {
  if (isUnauthorized(error)) {
    void queryClient.invalidateQueries({ queryKey: meQueryKey })
  }
}

export const queryClient: QueryClient = new QueryClient({
  queryCache: new QueryCache({
    onError: (error, query) => {
      if (query.queryKey[0] !== meQueryKey[0]) {
        handleUnauthorized(error)
      }
    },
  }),
  mutationCache: new MutationCache({ onError: handleUnauthorized }),
  defaultOptions: {
    queries: {
      // Client errors (401, 404, …) won't fix themselves on retry.
      retry: (failureCount, error) =>
        !(error instanceof ApiError && error.status < 500) && failureCount < 2,
    },
  },
})
