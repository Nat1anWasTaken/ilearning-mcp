-- The iLearning tokens become a link with the secrets `token` and, when
-- the sign-in link carried one, `privatetoken`. They were sealed the way
-- link secrets are (the user ID as associated data), so they move as is.
-- When the token was obtained moves into the link's metadata.
INSERT INTO links (user_id, status, token_hash, expires_at, last_ok_at, last_error, metadata,
                   created_at, updated_at)
SELECT user_id, status, token_hash, expires_at, last_ok_at, last_error,
       jsonb_build_object('token_obtained_at', token_obtained_at),
       token_obtained_at, updated_at
FROM ilearning_links;
INSERT INTO link_secrets (user_id, name, sealed)
SELECT user_id, 'token', token_sealed FROM ilearning_links
UNION ALL
SELECT user_id, 'privatetoken', privatetoken_sealed FROM ilearning_links
WHERE privatetoken_sealed IS NOT NULL;
DROP TABLE ilearning_links;
