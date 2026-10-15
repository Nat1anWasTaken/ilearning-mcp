import type { ReactNode } from "react"
import { Navigate, useLocation } from "react-router"

import { ErrorAlert } from "@/components/error-alert"
import { Spinner } from "@/components/ui/spinner"
import { useMe } from "@/hooks/use-me"
import { isUnauthorized } from "@/lib/api"

/** Renders `children` once `/api/me` loads; sends signed-out users to /login. */
export function RequireAuth({ children }: { children: ReactNode }) {
  const me = useMe()
  const location = useLocation()

  if (isUnauthorized(me.error)) {
    const next = location.pathname + location.search
    return <Navigate to={`/login?next=${encodeURIComponent(next)}`} replace />
  }
  // Keep showing the app if a background refetch fails but data is cached.
  if (me.data) {
    return children
  }
  if (me.isError) {
    return (
      <div className="flex min-h-svh items-center justify-center p-6">
        <ErrorAlert title="Couldn't reach the server" error={me.error} />
      </div>
    )
  }
  return (
    <div className="flex min-h-svh items-center justify-center">
      <Spinner />
    </div>
  )
}
