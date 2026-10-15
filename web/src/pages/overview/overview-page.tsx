import { Link } from "react-router"

import { PageHeader } from "@/components/page-header"
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar"
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
import { MCP_URL } from "@/lib/mcp-url"
import { CopyField } from "@/components/copy-field"
import { LinkCard } from "@/pages/overview/link-card"

export function OverviewPage() {
  return (
    <>
      <PageHeader title="Overview" />
      <ProfileCard />
      <LinkCard />
      <ConnectCard />
    </>
  )
}

function ProfileCard() {
  const { user } = useSignedInMe()
  return (
    <Card>
      <CardHeader className="flex items-center gap-4">
        <Avatar size="lg">
          {user.avatar_url && <AvatarImage src={user.avatar_url} alt="" />}
          <AvatarFallback>
            {user.fullname.slice(0, 1).toUpperCase()}
          </AvatarFallback>
        </Avatar>
        <div className="flex min-w-0 flex-col gap-1">
          <CardTitle className="truncate">{user.fullname}</CardTitle>
          <CardDescription>
            {user.username} · Moodle ID {user.moodle_userid}
          </CardDescription>
        </div>
      </CardHeader>
    </Card>
  )
}

function ConnectCard() {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Connect your AI</CardTitle>
        <CardDescription>
          Add this URL as a custom connector in claude.ai, ChatGPT, Claude Code
          or another MCP client.
        </CardDescription>
        <CardAction>
          <Button
            variant="outline"
            size="sm"
            render={<Link to="/connect" />}
            nativeButton={false}
          >
            Setup guide
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent>
        <CopyField label="MCP server URL" value={MCP_URL} />
      </CardContent>
    </Card>
  )
}
