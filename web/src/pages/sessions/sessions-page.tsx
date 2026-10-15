import { BotIcon } from "lucide-react"
import { Link } from "react-router"

import { ErrorAlert } from "@/components/error-alert"
import { PageHeader } from "@/components/page-header"
import { RelativeTime } from "@/components/relative-time"
import { Button } from "@/components/ui/button"
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty"
import { Skeleton } from "@/components/ui/skeleton"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"
import { useSessions } from "@/hooks/use-sessions"
import type { AgentSession } from "@/lib/api"
import { withQuery } from "@/lib/api"
import { RevokeSessionButton } from "@/pages/sessions/revoke-session-button"
import { SessionNameCell } from "@/pages/sessions/session-name-cell"

export function SessionsPage() {
  return (
    <>
      <PageHeader
        title="Connected clients"
        description="AI clients you've allowed to use your iLearning account."
      />
      <SessionsContent />
    </>
  )
}

function SessionsContent() {
  const sessions = useSessions()

  if (sessions.isPending) {
    return <Skeleton className="h-40 w-full" />
  }
  if (sessions.isError) {
    return (
      <ErrorAlert
        title="Couldn't load connected clients"
        error={sessions.error}
      />
    )
  }
  if (sessions.data.length === 0) {
    return <NoSessions />
  }
  return <SessionsTable sessions={sessions.data} />
}

function SessionsTable({ sessions }: { sessions: AgentSession[] }) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Name</TableHead>
          <TableHead>Client</TableHead>
          <TableHead>Connected</TableHead>
          <TableHead>Last used</TableHead>
          <TableHead className="text-right">Calls</TableHead>
          <TableHead>
            <span className="sr-only">Actions</span>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {sessions.map((session) => (
          <TableRow key={session.id}>
            <TableCell>
              <SessionNameCell session={session} />
            </TableCell>
            <TableCell>
              <div className="flex flex-col">
                <span>{session.client_name ?? session.client_id}</span>
                {session.origin && (
                  <span className="text-xs text-muted-foreground">
                    {session.origin}
                  </span>
                )}
              </div>
            </TableCell>
            <TableCell>
              <RelativeTime value={session.created_at} />
            </TableCell>
            <TableCell>
              {session.last_used_at ? (
                <RelativeTime value={session.last_used_at} />
              ) : (
                "Never"
              )}
            </TableCell>
            <TableCell className="text-right">
              <Button
                variant="link"
                className="px-0"
                render={
                  <Link
                    to={withQuery("/activity", { session_id: session.id })}
                  />
                }
                nativeButton={false}
              >
                {session.call_count}
              </Button>
            </TableCell>
            <TableCell className="text-right">
              <RevokeSessionButton session={session} />
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  )
}

function NoSessions() {
  return (
    <Empty>
      <EmptyHeader>
        <EmptyMedia variant="icon">
          <BotIcon />
        </EmptyMedia>
        <EmptyTitle>No clients connected</EmptyTitle>
        <EmptyDescription>
          Add the MCP server URL to an AI client and approve the connection to
          see it here.
        </EmptyDescription>
      </EmptyHeader>
      <EmptyContent>
        <Button render={<Link to="/connect" />} nativeButton={false}>
          Connect your AI
        </Button>
      </EmptyContent>
    </Empty>
  )
}
