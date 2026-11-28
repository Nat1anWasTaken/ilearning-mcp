import { ImageIcon } from "lucide-react"
import {
  mountDashboard,
  type NavItem,
  type User,
} from "@nat1an/mcp-backbone-dashboard"

import "./index.css"
import { IconsPage } from "@/icons-page"
import { ilearningLinkHealth } from "@/link-health"
import { Login, LoginDescription } from "@/login"
import { LinkCard } from "@/overview/link-card"
import { ProfileCard } from "@/overview/profile-card"

mountDashboard({
  name: "iLearning",
  logo: "/ilearning.png",
  logoClassName: "size-8",
  login: Login,
  loginDescription: <LoginDescription />,
  // The full name and the Moodle username (a student or staff ID).
  userDisplay: (user: User) => ({
    name: user.name ?? user.login ?? "Signed in",
    secondary: user.login,
    avatarUrl: user.avatar_url,
  }),
  linkHealth: ilearningLinkHealth,
  overviewCards: [ProfileCard, LinkCard],
  nav: [
    {
      to: "/icons",
      label: "Icons",
      // The cast bridges this app's lucide-react types and the package's,
      // which differ when the package is linked from a checkout.
      icon: ImageIcon as NavItem["icon"],
    },
  ],
  routes: [{ path: "icons", element: <IconsPage /> }],
  toolGroups: {
    profile: "Profile",
    courses: "Courses",
    assignments: "Assignments",
    grades: "Grades",
    forums: "Forums",
    messages: "Messages & notifications",
    files: "Files",
  },
  copy: { sessions: "Connected clients" },
})
