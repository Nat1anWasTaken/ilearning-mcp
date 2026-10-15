import type { ReactNode } from "react"

import { CopyField } from "@/components/copy-field"
import { PageHeader } from "@/components/page-header"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { MCP_URL } from "@/lib/mcp-url"

export function ConnectPage() {
  return (
    <>
      <PageHeader
        title="Connect your AI"
        description="Add this server to your AI client. You'll be asked to sign in and approve the connection here."
      />
      <CopyField label="MCP server URL" value={MCP_URL} />
      <Tabs defaultValue="claude-ai">
        <TabsList>
          <TabsTrigger value="claude-ai">claude.ai</TabsTrigger>
          <TabsTrigger value="chatgpt">ChatGPT</TabsTrigger>
          <TabsTrigger value="claude-code">Claude Code</TabsTrigger>
          <TabsTrigger value="other">Gemini & others</TabsTrigger>
        </TabsList>
        <TabsContent value="claude-ai">
          <Instructions
            title="claude.ai"
            description="Also works in the Claude desktop and mobile apps."
          >
            <li>Open Settings → Connectors.</li>
            <li>Click “Add custom connector”.</li>
            <li>Paste the MCP server URL above.</li>
            <li>Leave the OAuth Client ID and Client Secret fields empty.</li>
            <li>Click Add, then Connect, and approve the request here.</li>
          </Instructions>
        </TabsContent>
        <TabsContent value="chatgpt">
          <Instructions
            title="ChatGPT"
            description="Custom connectors require developer mode."
          >
            <li>Open Settings → Apps & Connectors → Advanced settings.</li>
            <li>Turn on Developer mode.</li>
            <li>
              Back in Apps & Connectors, click Create and name it iLearning.
            </li>
            <li>
              Paste the MCP server URL above and choose OAuth authentication.
            </li>
            <li>Click Create, then sign in and approve the request here.</li>
          </Instructions>
        </TabsContent>
        <TabsContent value="claude-code">
          <Instructions
            title="Claude Code"
            description="Run this in your terminal, then use /mcp inside Claude Code to authenticate."
          >
            <li>
              <CopyField
                label="Claude Code command"
                value={`claude mcp add --transport http ilearning ${MCP_URL}`}
              />
            </li>
            <li>Start Claude Code and run /mcp.</li>
            <li>
              Choose “ilearning”, authenticate, and approve the request here.
            </li>
          </Instructions>
        </TabsContent>
        <TabsContent value="other">
          <Instructions
            title="Gemini and other MCP clients"
            description="Any client that supports remote MCP servers over Streamable HTTP with OAuth works."
          >
            <li>
              Gemini CLI: run this command, then /mcp auth ilearning.
              <CopyField
                label="Gemini CLI command"
                value={`gemini mcp add --transport http ilearning ${MCP_URL}`}
              />
            </li>
            <li>
              Other clients: add a remote (HTTP) MCP server in their settings.
            </li>
            <li>Use the MCP server URL above.</li>
            <li>
              Choose OAuth if asked. The client registers itself automatically;
              no client ID or secret is needed.
            </li>
            <li>Approve the request here when your browser opens.</li>
          </Instructions>
        </TabsContent>
      </Tabs>
    </>
  )
}

type InstructionsProps = {
  title: string
  description: string
  children: ReactNode
}

function Instructions({ title, description, children }: InstructionsProps) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent>
        <ol className="flex list-decimal flex-col gap-2 pl-5">{children}</ol>
      </CardContent>
    </Card>
  )
}
