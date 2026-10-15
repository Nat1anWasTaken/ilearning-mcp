import type { ReactNode } from "react"
import { Link } from "react-router"

import { LinkStatusBadge } from "@/components/link-status-badge"
import { RelativeTime } from "@/components/relative-time"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { useSignedInMe } from "@/hooks/use-me"

export function LinkCard() {
  const { link } = useSignedInMe()

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          iLearning link
          <LinkStatusBadge status={link.status} />
        </CardTitle>
        <CardDescription>
          The iLearning token your agents use. 代理程式使用的 iLearning 憑證。
        </CardDescription>
        <CardAction>
          <Button
            variant={link.status === "needs_relink" ? "default" : "outline"}
            size="sm"
            render={<Link to="/login?next=/" />}
            nativeButton={false}
          >
            Re-link
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <dl className="grid gap-x-6 gap-y-3 text-sm sm:grid-cols-[auto_1fr]">
          <Row label="Token obtained">
            <RelativeTime value={link.token_obtained_at} />
          </Row>
          <Row label="Expires">
            {link.expires_at ? (
              <RelativeTime value={link.expires_at} />
            ) : (
              "Unknown"
            )}
          </Row>
          <Row label="Last successful call">
            {link.last_ok_at ? (
              <RelativeTime value={link.last_ok_at} />
            ) : (
              "Never"
            )}
          </Row>
          <Row label="Auto-renew">{link.auto_renew ? "Yes" : "No"}</Row>
          {link.last_error && (
            <Row label="Last error">
              <span className="text-destructive">{link.last_error}</span>
            </Row>
          )}
        </dl>
        <p className="text-sm text-muted-foreground">
          {link.auto_renew
            ? "The token is renewed automatically before it expires. Renewal briefly signs you out of iLearning in your browser, about once every 12 weeks."
            : "Automatic renewal isn't available for this link (its expiry is unknown), so you'll need to re-link when it stops working."}
        </p>
      </CardContent>
    </Card>
  )
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-muted-foreground">{label}</dt>
      <dd>{children}</dd>
    </>
  )
}
