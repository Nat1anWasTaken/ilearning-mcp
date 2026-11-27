// Shapes of the server's iLearning-specific API (see docs/ARCHITECTURE.md).

/** `/api/me`'s `link`: the iLearning token agents use, or null. */
export type LinkStatus = {
  status: "ok" | "renewing" | "needs_relink"
  token_obtained_at: string
  /** null = unknown */
  expires_at: string | null
  last_ok_at: string | null
  last_error: string | null
  /** Has a private token and a known expiry. */
  auto_renew: boolean
} | null

/** `POST /auth/ilearning/start`. */
export type LinkStart = {
  attempt_id: string
  launch_url: string
  expires_at: string
}

/** Error codes of `POST /auth/ilearning/complete`. */
export type LinkErrorCode =
  | "invalid_link"
  | "link_expired"
  | "not_allowed"
  | "wrong_account"
  | "moodle_error"
