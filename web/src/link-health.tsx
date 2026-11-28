import { Link } from "react-router"

import type { LinkHealth, Me } from "@nat1an/mcp-backbone-dashboard"
import { Button } from "@nat1an/mcp-backbone-dashboard/ui/button"

import type { LinkStatus } from "@/lib/types"

/** Restarts the two-step iLearning sign-in, then comes back here. */
export function RelinkButton({
  variant = "default",
  children = "Re-link",
}: {
  variant?: "default" | "outline" | "destructive"
  children?: string
}) {
  return (
    <Button
      size="sm"
      variant={variant}
      render={<Link to="/login?next=/" />}
      nativeButton={false}
    >
      {children}
    </Button>
  )
}

/** Whether agents can reach iLearning with the stored token. */
export function ilearningLinkHealth(me: Me<unknown>): LinkHealth {
  const link = me.link as LinkStatus
  if (!link) {
    return {
      ok: false,
      title: "Your iLearning account isn't linked",
      description:
        "Agents can't reach iLearning until you link it. 請連結 iLearning 帳號。",
      action: <RelinkButton variant="destructive">Link</RelinkButton>,
    }
  }
  if (link.status === "needs_relink") {
    return {
      ok: false,
      title: "Your iLearning link has stopped working",
      description:
        "Agents can't reach iLearning until you link it again. 請重新連結 iLearning 帳號。",
      action: <RelinkButton variant="destructive" />,
    }
  }
  return { ok: true }
}
