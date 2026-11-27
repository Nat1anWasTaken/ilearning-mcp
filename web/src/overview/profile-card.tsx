import { initials, useSignedInMe } from "@nat1an/mcp-backbone-dashboard"
import {
  Avatar,
  AvatarFallback,
  AvatarImage,
} from "@nat1an/mcp-backbone-dashboard/ui/avatar"
import {
  Card,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@nat1an/mcp-backbone-dashboard/ui/card"

/** The signed-in iLearning account. */
export function ProfileCard() {
  const { user } = useSignedInMe()
  const name = user.name ?? user.login ?? user.subject
  // `subject` is the Moodle user ID.
  const details = [user.login, `Moodle ID ${user.subject}`]
    .filter(Boolean)
    .join(" · ")

  return (
    <Card>
      <CardHeader className="flex items-center gap-4">
        <Avatar size="lg">
          {user.avatar_url && <AvatarImage src={user.avatar_url} alt="" />}
          <AvatarFallback>{initials(name)}</AvatarFallback>
        </Avatar>
        <div className="flex min-w-0 flex-col gap-1">
          <CardTitle className="truncate">{name}</CardTitle>
          <CardDescription>{details}</CardDescription>
        </div>
      </CardHeader>
    </Card>
  )
}
