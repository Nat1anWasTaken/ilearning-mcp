import type { ComponentProps } from "react"

import { Badge } from "@nat1an/mcp-backbone-dashboard/ui/badge"

import type { LinkStatus } from "@/lib/types"

type BadgeVariant = ComponentProps<typeof Badge>["variant"]

const LINK_STATUSES: Record<
  NonNullable<LinkStatus>["status"],
  { label: string; variant: BadgeVariant }
> = {
  ok: { label: "Linked", variant: "secondary" },
  renewing: { label: "Renewing", variant: "outline" },
  needs_relink: { label: "Needs re-link", variant: "destructive" },
}

export function LinkStatusBadge({ link }: { link: LinkStatus }) {
  const { label, variant } = link
    ? LINK_STATUSES[link.status]
    : { label: "Not linked", variant: "destructive" as const }
  return <Badge variant={variant}>{label}</Badge>
}
