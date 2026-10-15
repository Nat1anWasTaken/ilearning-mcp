import { DownloadIcon } from "lucide-react"

import { CopyField } from "@/components/copy-field"
import { PageHeader } from "@/components/page-header"
import { Button } from "@/components/ui/button"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"

type IconFile = { path: string; format: string; size: string }

const PNG_SIZES = [16, 32, 48, 64, 128, 256, 512]

const ICON_FILES: IconFile[] = [
  { path: "/icons/ilearning.svg", format: "SVG", size: "Any" },
  ...PNG_SIZES.map((size) => ({
    path: `/icons/ilearning-${size}.png`,
    format: "PNG",
    size: `${size} × ${size}`,
  })),
  { path: "/icons/ilearning.ico", format: "ICO", size: "16 – 256" },
]

export function IconsPage() {
  return (
    <>
      <PageHeader
        title="Icons"
        description="Download the iLearning icon to use for this server in your AI client. Clients that ask for an icon URL can use the link directly."
      />
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Preview</TableHead>
            <TableHead>Format</TableHead>
            <TableHead>Size</TableHead>
            <TableHead>URL</TableHead>
            <TableHead>
              <span className="sr-only">Download</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {ICON_FILES.map((icon) => (
            <IconRow key={icon.path} icon={icon} />
          ))}
        </TableBody>
      </Table>
    </>
  )
}

function IconRow({ icon }: { icon: IconFile }) {
  const url = `${window.location.origin}${icon.path}`
  const filename = icon.path.split("/").pop()

  return (
    <TableRow>
      <TableCell>
        <img src={icon.path} alt="" className="size-8" />
      </TableCell>
      <TableCell>{icon.format}</TableCell>
      <TableCell>{icon.size}</TableCell>
      <TableCell className="w-full min-w-64">
        <CopyField label={`${icon.format} ${icon.size} icon URL`} value={url} />
      </TableCell>
      <TableCell className="text-right">
        <Button
          variant="outline"
          size="sm"
          render={<a href={icon.path} download={filename} />}
          nativeButton={false}
        >
          <DownloadIcon />
          Download
        </Button>
      </TableCell>
    </TableRow>
  )
}
