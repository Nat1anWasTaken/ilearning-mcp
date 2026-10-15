import {
  ActivityIcon,
  BotIcon,
  HouseIcon,
  PlugIcon,
  type LucideIcon,
} from "lucide-react"
import { matchPath, NavLink, useLocation } from "react-router"

import { NavUser } from "@/components/nav-user"
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
} from "@/components/ui/sidebar"

type NavItem = { to: string; label: string; icon: LucideIcon; end?: boolean }

const NAV_ITEMS: NavItem[] = [
  { to: "/", label: "Overview", icon: HouseIcon, end: true },
  { to: "/sessions", label: "Connected clients", icon: BotIcon },
  { to: "/activity", label: "Activity", icon: ActivityIcon },
  { to: "/connect", label: "Connect", icon: PlugIcon },
]

export function AppSidebar() {
  const { pathname } = useLocation()

  return (
    <Sidebar>
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton size="lg" render={<NavLink to="/" />}>
              <img src="/ilearning.svg" alt="" className="size-8" />
              <span className="font-semibold">iLearning MCP</span>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>
      <SidebarContent>
        <SidebarGroup>
          <SidebarGroupContent>
            <SidebarMenu>
              {NAV_ITEMS.map(({ to, label, icon: Icon, end = false }) => (
                <SidebarMenuItem key={to}>
                  <SidebarMenuButton
                    isActive={matchPath({ path: to, end }, pathname) !== null}
                    render={<NavLink to={to} end={end} />}
                  >
                    <Icon />
                    <span>{label}</span>
                  </SidebarMenuButton>
                </SidebarMenuItem>
              ))}
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>
      </SidebarContent>
      <SidebarFooter>
        <NavUser />
      </SidebarFooter>
    </Sidebar>
  )
}
