import { createBrowserRouter, Navigate } from "react-router"

import { AppLayout } from "@/components/app-layout"
import { ActivityPage } from "@/pages/activity/activity-page"
import { ConnectPage } from "@/pages/connect/connect-page"
import { ConsentPage } from "@/pages/consent/consent-page"
import { IconsPage } from "@/pages/icons/icons-page"
import { LoginPage } from "@/pages/login/login-page"
import { OverviewPage } from "@/pages/overview/overview-page"
import { SessionsPage } from "@/pages/sessions/sessions-page"

export const router = createBrowserRouter([
  { path: "/login", element: <LoginPage /> },
  { path: "/consent/:id", element: <ConsentPage /> },
  {
    element: <AppLayout />,
    children: [
      { index: true, element: <OverviewPage /> },
      { path: "sessions", element: <SessionsPage /> },
      { path: "activity", element: <ActivityPage /> },
      { path: "connect", element: <ConnectPage /> },
      { path: "icons", element: <IconsPage /> },
      { path: "*", element: <Navigate to="/" replace /> },
    ],
  },
])
