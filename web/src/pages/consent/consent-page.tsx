import { RequireAuth } from "@/components/require-auth"
import { ErrorAlert } from "@/components/error-alert"
import { Skeleton } from "@/components/ui/skeleton"
import { useOAuthRequest } from "@/hooks/use-oauth-request"
import { useRequiredParam } from "@/hooks/use-required-param"
import { ApiError } from "@/lib/api"
import { ConsentForm } from "@/pages/consent/consent-form"

/** OAuth consent screen, shown outside the dashboard layout. */
export function ConsentPage() {
  return (
    <RequireAuth>
      <div className="flex min-h-svh items-center justify-center p-6">
        <div className="w-full max-w-md">
          <ConsentContent />
        </div>
      </div>
    </RequireAuth>
  )
}

function ConsentContent() {
  const id = useRequiredParam("id")
  const request = useOAuthRequest(id)

  if (request.isPending) {
    return <Skeleton className="h-96 w-full" />
  }
  if (request.isError) {
    const expired =
      request.error instanceof ApiError && request.error.status === 404
    return (
      <ErrorAlert
        title={
          expired
            ? "This authorization request has expired. Start again from your AI client."
            : "Couldn't load this authorization request"
        }
        error={request.error}
      />
    )
  }
  return <ConsentForm requestId={id} request={request.data} />
}
