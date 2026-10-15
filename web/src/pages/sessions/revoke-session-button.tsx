import { toast } from "sonner"

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog"
import { Button } from "@/components/ui/button"
import { useDeleteSession } from "@/hooks/use-sessions"
import type { AgentSession } from "@/lib/api"

export function RevokeSessionButton({ session }: { session: AgentSession }) {
  const deleteSession = useDeleteSession(session.id)

  function revoke() {
    deleteSession.mutate(undefined, {
      onSuccess: () => toast.success(`Revoked ${session.name}`),
      onError: (error) => toast.error(error.message),
    })
  }

  return (
    <AlertDialog>
      <AlertDialogTrigger render={<Button variant="outline" size="sm" />}>
        Revoke
      </AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Revoke {session.name}?</AlertDialogTitle>
          <AlertDialogDescription>
            The client loses access to your iLearning account immediately. Its
            activity log is kept. To use it again, reconnect it from the AI
            client.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            disabled={deleteSession.isPending}
            onClick={revoke}
          >
            Revoke
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
