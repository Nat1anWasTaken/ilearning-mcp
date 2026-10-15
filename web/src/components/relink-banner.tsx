import { CircleAlertIcon } from "lucide-react"
import { Link } from "react-router"

import {
  Alert,
  AlertAction,
  AlertDescription,
  AlertTitle,
} from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import { useSignedInMe } from "@/hooks/use-me"

/** Shown on every dashboard page while the iLearning link is broken. */
export function RelinkBanner() {
  const { link } = useSignedInMe()
  if (link.status !== "needs_relink") {
    return null
  }
  return (
    <Alert variant="destructive">
      <CircleAlertIcon />
      <AlertTitle>Your iLearning link has stopped working</AlertTitle>
      <AlertDescription>
        Agents can't reach iLearning until you link it again. 請重新連結
        iLearning 帳號。
      </AlertDescription>
      <AlertAction>
        <Button
          size="sm"
          variant="destructive"
          render={<Link to="/login?next=/" />}
          nativeButton={false}
        >
          Re-link
        </Button>
      </AlertAction>
    </Alert>
  )
}
