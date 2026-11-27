import { useEffect, useRef } from "react"
import { RotateCcwIcon } from "lucide-react"
import { useSearchParams } from "react-router"

import { ErrorAlert, nextPath, useMe } from "@nat1an/mcp-backbone-dashboard"
import { Button } from "@nat1an/mcp-backbone-dashboard/ui/button"
import { Skeleton } from "@nat1an/mcp-backbone-dashboard/ui/skeleton"

import { useStartLink } from "@/hooks/use-ilearning-link"
import { LinkSteps } from "@/link-steps"

/**
 * The login page's content: sign in by linking iLearning (the iLearning
 * account *is* the user). Also used to re-link while signed in.
 */
export function Login() {
  const [searchParams] = useSearchParams()
  const next = nextPath(searchParams)
  const start = useStartLink()
  const { mutate } = start

  // Start one attempt on mount (the ref keeps StrictMode from starting two:
  // each attempt replaces the pre-auth cookie of the previous one).
  const started = useRef(false)
  useEffect(() => {
    if (!started.current) {
      started.current = true
      mutate(next)
    }
  }, [mutate, next])

  const restart = () => start.mutate(next)

  if (start.isError) {
    return (
      <>
        <ErrorAlert title="Couldn't start signing in" error={start.error} />
        <Button variant="outline" onClick={restart}>
          <RotateCcwIcon />
          Try again
        </Button>
      </>
    )
  }
  if (start.isSuccess) {
    // Keyed so a restart resets the pasted text and errors.
    return (
      <LinkSteps
        key={start.data.attempt_id}
        attempt={start.data}
        onRestart={restart}
      />
    )
  }
  return (
    <div className="flex flex-col gap-3">
      <Skeleton className="h-16 w-full" />
      <Skeleton className="h-16 w-full" />
      <Skeleton className="h-28 w-full" />
    </div>
  )
}

/** The line under the login title: signing in, or re-linking if signed in. */
export function LoginDescription() {
  const me = useMe()
  return (
    <span className="flex flex-col gap-0.5">
      {me.data ? (
        <>
          <span>重新連結您的 iLearning 帳號。</span>
          <span>
            Re-link your iLearning account
            {me.data.user.login && ` (${me.data.user.login})`}.
          </span>
        </>
      ) : (
        <>
          <span>使用 iLearning 帳號登入，讓 AI 助理代您使用 iLearning。</span>
          <span>
            Sign in with iLearning to let AI agents use it on your behalf.
          </span>
        </>
      )}
    </span>
  )
}
