import { useState, type FormEvent } from "react"
import { TriangleAlertIcon } from "lucide-react"

import { ErrorAlert } from "@/components/error-alert"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import {
  Field,
  FieldDescription,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field"
import { Input } from "@/components/ui/input"
import { Spinner } from "@/components/ui/spinner"
import { useSignedInMe } from "@/hooks/use-me"
import { useDecideOAuthRequest } from "@/hooks/use-oauth-request"
import type { OAuthRequest } from "@/lib/api"

type ConsentFormProps = {
  requestId: string
  request: OAuthRequest
}

export function ConsentForm({ requestId, request }: ConsentFormProps) {
  const { user, link } = useSignedInMe()
  const decide = useDecideOAuthRequest(requestId)
  const clientName = request.client_name ?? request.origin ?? "An AI agent"
  const existing = request.existing_session
  const [name, setName] = useState(existing?.name ?? clientName)

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    decide.mutate({
      approve: true,
      name: name.trim() || clientName,
    })
  }

  // Stay busy after success too: the browser is navigating to the client.
  const busy = decide.isPending || decide.isSuccess
  const approving = decide.variables?.approve

  return (
    <form onSubmit={handleSubmit}>
      <Card>
        <CardHeader>
          <Avatar size="lg">
            {request.logo_uri && <AvatarImage src={request.logo_uri} alt="" />}
            <AvatarFallback>
              {clientName.slice(0, 1).toUpperCase()}
            </AvatarFallback>
          </Avatar>
          <CardTitle>
            {clientName} wants to use your iLearning account
          </CardTitle>
          <CardDescription>
            <ClientDetails request={request} />
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-6">
          {link.status === "needs_relink" && (
            <Alert>
              <TriangleAlertIcon />
              <AlertTitle>Your iLearning link needs attention</AlertTitle>
              <AlertDescription>
                You can approve now, but the client won't be able to reach
                iLearning until you re-link on the dashboard.
              </AlertDescription>
            </Alert>
          )}
          <p className="text-sm text-muted-foreground">
            Signed in as <strong>{user.fullname}</strong> ({user.username}). The
            client will be able to read your courses, assignments, grades,
            forums and messages, and act on your behalf (submit assignments,
            post to forums, send messages).
          </p>
          <FieldGroup>
            <Field>
              <FieldLabel htmlFor="consent-name">Session name</FieldLabel>
              <Input
                id="consent-name"
                value={name}
                onChange={(event) => setName(event.target.value)}
              />
              <FieldDescription>
                How this connection appears on the dashboard.
              </FieldDescription>
            </Field>
          </FieldGroup>
          {decide.isError && <ErrorAlert error={decide.error} />}
        </CardContent>
        <CardFooter className="justify-end gap-2">
          <Button
            type="button"
            variant="outline"
            disabled={busy}
            onClick={() => decide.mutate({ approve: false })}
          >
            {busy && approving === false && <Spinner />}
            Deny
          </Button>
          <Button type="submit" disabled={busy}>
            {busy && approving === true && <Spinner />}
            Allow
          </Button>
        </CardFooter>
      </Card>
    </form>
  )
}

function ClientDetails({ request }: { request: OAuthRequest }) {
  const redirectHost =
    URL.parse(request.redirect_uri)?.host || request.redirect_uri

  return (
    <span className="flex flex-col gap-1">
      {request.origin && <span>From {request.origin}</span>}
      {request.client_uri && (
        <a href={request.client_uri} target="_blank" rel="noreferrer">
          {request.client_uri}
        </a>
      )}
      <span>
        After you decide, you'll be sent to <strong>{redirectHost}</strong>.
      </span>
    </span>
  )
}
