import { useState, type FormEvent } from "react"
import { CheckIcon, PencilIcon, XIcon } from "lucide-react"

import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { useRenameSession } from "@/hooks/use-sessions"
import type { AgentSession } from "@/lib/api"

/** The session name, editable in place. */
export function SessionNameCell({ session }: { session: AgentSession }) {
  const rename = useRenameSession(session.id)
  const [draft, setDraft] = useState<string | null>(null)

  if (draft === null) {
    return (
      <div className="flex items-center gap-1">
        <span className="font-medium">{session.name}</span>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label={`Rename ${session.name}`}
          onClick={() => setDraft(session.name)}
        >
          <PencilIcon />
        </Button>
      </div>
    )
  }

  const trimmed = draft.trim()

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!trimmed || trimmed === session.name) {
      setDraft(null)
      return
    }
    // Keep the draft on failure so the user can retry.
    rename.mutate(trimmed, { onSuccess: () => setDraft(null) })
  }

  return (
    <form className="flex items-center gap-1" onSubmit={handleSubmit}>
      <Input
        aria-label="Session name"
        className="h-7 w-48"
        autoFocus
        required
        value={draft}
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={(event) => event.key === "Escape" && setDraft(null)}
      />
      <Button
        type="submit"
        variant="ghost"
        size="icon-xs"
        aria-label="Save name"
        disabled={rename.isPending || !trimmed}
      >
        <CheckIcon />
      </Button>
      <Button
        type="button"
        variant="ghost"
        size="icon-xs"
        aria-label="Cancel"
        onClick={() => setDraft(null)}
      >
        <XIcon />
      </Button>
    </form>
  )
}
