import { useRef, useState, type FormEvent, type ReactNode } from "react"
import {
  CircleAlertIcon,
  ClipboardPasteIcon,
  ExternalLinkIcon,
  RotateCcwIcon,
  TimerIcon,
} from "lucide-react"
import { toast } from "sonner"

import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import { Spinner } from "@/components/ui/spinner"
import { Textarea } from "@/components/ui/textarea"
import { useCompleteLink } from "@/hooks/use-ilearning-link"
import { useLogout } from "@/hooks/use-me"
import { useNow } from "@/hooks/use-now"
import { ApiError, type LinkErrorCode, type LinkStart } from "@/lib/api"
import { formatCountdown } from "@/lib/format"

const LINK_TEXT = "若應用程式沒有自動開啟，請點選這裡"

type LinkStepsProps = {
  attempt: LinkStart
  onRestart: () => void
}

export function LinkSteps({ attempt, onRestart }: LinkStepsProps) {
  const complete = useCompleteLink()
  const [pasted, setPasted] = useState("")
  const textareaRef = useRef<HTMLTextAreaElement>(null)
  const now = useNow()
  const remaining = new Date(attempt.expires_at).getTime() - now
  const expired = remaining <= 0
  // Stay busy after success too: the browser is navigating away.
  const busy = complete.isPending || complete.isSuccess
  const canReadClipboard =
    typeof navigator !== "undefined" && !!navigator.clipboard?.readText

  function submit(value: string) {
    const trimmed = value.trim()
    if (!trimmed || busy) {
      return
    }
    complete.mutate({ attempt_id: attempt.attempt_id, pasted: trimmed })
  }

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    submit(pasted)
  }

  async function pasteFromClipboard() {
    try {
      const text = await navigator.clipboard.readText()
      if (!text.trim()) {
        toast.error("剪貼簿是空的 · The clipboard is empty")
        return
      }
      setPasted(text)
      // A copied app link is unambiguous: go ahead.
      if (text.trim().startsWith("moodlemobile://")) {
        submit(text)
      }
    } catch {
      toast.error("無法讀取剪貼簿 · Couldn't read the clipboard", {
        description: "Paste into the box instead (Ctrl/⌘+V or long-press).",
      })
      textareaRef.current?.focus()
    }
  }

  if (expired && !busy) {
    return (
      <div className="flex flex-col gap-4">
        <Alert>
          <TimerIcon />
          <AlertTitle>
            此登入連結已過期 · This sign-in attempt expired
          </AlertTitle>
          <AlertDescription>
            Start again to get a fresh iLearning link.
          </AlertDescription>
        </Alert>
        <Button onClick={onRestart}>
          <RotateCcwIcon />
          重新開始 · Start again
        </Button>
      </div>
    )
  }

  return (
    <form className="flex flex-col gap-5" onSubmit={handleSubmit}>
      <ol className="flex flex-col gap-5">
        <Step number={1} title="開啟 iLearning" subtitle="Open iLearning">
          <p className="text-sm text-muted-foreground">
            如被要求，請使用 Microsoft 365 帳號登入。
            <br />
            Sign in with Microsoft 365 if asked.
          </p>
          <Button
            className="self-start"
            render={
              <a href={attempt.launch_url} target="_blank" rel="noreferrer" />
            }
            nativeButton={false}
          >
            <ExternalLinkIcon />
            開啟 iLearning · Open iLearning
          </Button>
        </Step>
        <Step number={2} title="複製連結" subtitle="Copy the link">
          <p className="text-sm">
            在出現的頁面上，於「
            <strong>{LINK_TEXT}</strong>
            」按右鍵（電腦）或長按（手機），選擇「複製連結」。
          </p>
          <p className="text-sm text-muted-foreground">
            On the page that appears, right-click (desktop) or long-press
            (phone) the link “{LINK_TEXT}” (“click here to launch the app”) and
            choose Copy link.
          </p>
          <p className="text-xs text-muted-foreground">
            若先出現「無法開啟」或「要開啟應用程式嗎？」的提示，請關閉它。
            <br />A “can't open this page” or “open app?” prompt may appear
            first: dismiss it.
          </p>
        </Step>
        <Step number={3} title="貼上連結" subtitle="Paste it here">
          <Textarea
            ref={textareaRef}
            aria-label="Copied link"
            placeholder="moodlemobile://token=…"
            className="min-h-20 font-mono text-xs break-all"
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            spellCheck={false}
            value={pasted}
            disabled={busy}
            onChange={(event) => setPasted(event.target.value)}
          />
          <div className="flex flex-wrap gap-2">
            {canReadClipboard && (
              <Button
                type="button"
                variant="outline"
                disabled={busy}
                onClick={() => void pasteFromClipboard()}
              >
                <ClipboardPasteIcon />
                從剪貼簿貼上 · Paste from clipboard
              </Button>
            )}
            <Button type="submit" disabled={busy || !pasted.trim()}>
              {busy && <Spinner />}
              登入 · Sign in
            </Button>
          </div>
        </Step>
      </ol>
      {complete.isError && (
        <CompleteError error={complete.error} onRestart={onRestart} />
      )}
      <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
        <span className="flex items-center gap-1">
          <TimerIcon className="size-3.5" />
          剩餘 {formatCountdown(remaining)} · expires in{" "}
          {formatCountdown(remaining)}
        </span>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={busy}
          onClick={onRestart}
        >
          <RotateCcwIcon />
          重新開始 · Restart
        </Button>
      </div>
    </form>
  )
}

type StepProps = {
  number: number
  title: string
  subtitle: string
  children: ReactNode
}

function Step({ number, title, subtitle, children }: StepProps) {
  return (
    <li className="flex gap-3">
      <span className="flex size-7 shrink-0 items-center justify-center rounded-full bg-primary text-sm font-medium text-primary-foreground">
        {number}
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        <div className="flex flex-col">
          <span className="font-medium">{title}</span>
          <span className="text-sm text-muted-foreground">{subtitle}</span>
        </div>
        {children}
      </div>
    </li>
  )
}

const LINK_ERRORS: Record<LinkErrorCode, { zh: string; en: string }> = {
  invalid_link: {
    zh:
      "這不是正確的連結。請複製「" +
      LINK_TEXT +
      "」的連結，而不是網址列的網址。",
    en: "That isn't the right link. Copy the “click here to launch the app” link itself (it starts with moodlemobile://), not the page address.",
  },
  link_expired: {
    zh: "此登入連結已過期，請重新開始。",
    en: "This sign-in attempt expired or was started in another tab. Start again.",
  },
  not_allowed: {
    zh: "此 iLearning 帳號無法使用本服務。",
    en: "This iLearning account isn't allowed to use this server.",
  },
  wrong_account: {
    zh: "這個連結屬於另一個 iLearning 帳號。",
    en: "This link belongs to a different iLearning account than the one you're signed in with. Sign out first to switch accounts.",
  },
  moodle_error: {
    zh: "iLearning 發生錯誤，請稍後再試。",
    en: "iLearning returned an error. Try again in a moment.",
  },
}

function CompleteError({
  error,
  onRestart,
}: {
  error: Error
  onRestart: () => void
}) {
  const logout = useLogout()
  const code =
    error instanceof ApiError && Object.hasOwn(LINK_ERRORS, error.code)
      ? (error.code as LinkErrorCode)
      : null

  if (code === null) {
    return (
      <Alert variant="destructive">
        <CircleAlertIcon />
        <AlertTitle>登入失敗 · Couldn't sign in</AlertTitle>
        <AlertDescription>{error.message}</AlertDescription>
      </Alert>
    )
  }

  const { zh, en } = LINK_ERRORS[code]
  return (
    <Alert variant="destructive">
      <CircleAlertIcon />
      <AlertTitle>{zh}</AlertTitle>
      <AlertDescription className="flex flex-col gap-2">
        <span>{en}</span>
        {code === "moodle_error" && error.message && (
          <span className="text-xs">{error.message}</span>
        )}
        {code === "link_expired" && (
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="self-start"
            onClick={onRestart}
          >
            <RotateCcwIcon />
            重新開始 · Start again
          </Button>
        )}
        {code === "wrong_account" && (
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="self-start"
            disabled={logout.isPending}
            onClick={() => logout.mutate()}
          >
            登出 · Sign out
          </Button>
        )}
      </AlertDescription>
    </Alert>
  )
}
