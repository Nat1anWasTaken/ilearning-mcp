import { Outlet } from "react-router"

import { AppSidebar } from "@/components/app-sidebar"
import { RelinkBanner } from "@/components/relink-banner"
import { RequireAuth } from "@/components/require-auth"
import {
  SidebarInset,
  SidebarProvider,
  SidebarTrigger,
} from "@/components/ui/sidebar"

export function AppLayout() {
  return (
    <RequireAuth>
      <SidebarProvider>
        <AppSidebar />
        <SidebarInset>
          <header className="flex h-12 items-center gap-2 px-4">
            <SidebarTrigger />
          </header>
          <div className="flex w-full max-w-5xl flex-col gap-6 p-4 md:p-6">
            <RelinkBanner />
            <Outlet />
          </div>
        </SidebarInset>
      </SidebarProvider>
    </RequireAuth>
  )
}
