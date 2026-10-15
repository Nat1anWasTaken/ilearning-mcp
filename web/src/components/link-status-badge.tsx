import type { ComponentProps } from "react"

import { Badge } from "@/components/ui/badge"
import type { LinkStatus } from "@/lib/api"

type BadgeVariant = ComponentProps<typeof Badge>["variant"]

const LINK_STATUSES: Record<
  LinkStatus["status"],
  { label: string; variant: BadgeVariant }
> = {
  ok: { label: "Linked", variant: "secondary" },
  renewing: { label: "Renewing", variant: "outline" },
  needs_relink: { label: "Needs re-link", variant: "destructive" },
}

export function LinkStatusBadge({ status }: { status: LinkStatus["status"] }) {
  const { label, variant } = LINK_STATUSES[status]
  return <Badge variant={variant}>{label}</Badge>
}
