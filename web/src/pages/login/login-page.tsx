import { useEffect, useRef } from "react"
import { RotateCcwIcon } from "lucide-react"
import { useSearchParams } from "react-router"

import { ErrorAlert } from "@/components/error-alert"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Skeleton } from "@/components/ui/skeleton"
import { useStartLink } from "@/hooks/use-ilearning-link"
import { useMe } from "@/hooks/use-me"
import { LinkSteps } from "@/pages/login/link-steps"

/**
 * Sign in by linking iLearning: the iLearning account *is* the user. Also
 * used to re-link while signed in.
 */
export function LoginPage() {
  const [searchParams] = useSearchParams()
  const next = searchParams.get("next") ?? "/"
  const me = useMe()
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
  const signedIn = me.data !== undefined

  return (
    <div className="flex min-h-svh items-center justify-center p-4 sm:p-6">
      <Card className="w-full max-w-lg">
        <CardHeader>
          <CardTitle className="flex items-center gap-2">
            <img src="/ilearning.svg" alt="" className="size-8" />
            iLearning MCP
          </CardTitle>
          <CardDescription className="flex flex-col gap-0.5">
            {signedIn ? (
              <>
                <span>重新連結您的 iLearning 帳號。</span>
                <span>
                  Re-link your iLearning account
                  {me.data && ` (${me.data.user.username})`}.
                </span>
              </>
            ) : (
              <>
                <span>
                  使用 iLearning 帳號登入，讓 AI 助理代您使用 iLearning。
                </span>
                <span>
                  Sign in with iLearning to let AI agents use it on your behalf.
                </span>
              </>
            )}
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          {(start.isPending || start.isIdle) && (
            <div className="flex flex-col gap-3">
              <Skeleton className="h-16 w-full" />
              <Skeleton className="h-16 w-full" />
              <Skeleton className="h-28 w-full" />
            </div>
          )}
          {start.isError && (
            <>
              <ErrorAlert
                title="Couldn't start signing in"
                error={start.error}
              />
              <Button variant="outline" onClick={restart}>
                <RotateCcwIcon />
                Try again
              </Button>
            </>
          )}
          {start.isSuccess && (
            // Keyed so a restart resets the pasted text and errors.
            <LinkSteps
              key={start.data.attempt_id}
              attempt={start.data}
              onRestart={restart}
            />
          )}
        </CardContent>
      </Card>
    </div>
  )
}
