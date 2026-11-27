-- Adopts an ilearning-mcp database from before mcp-backbone: reshapes the
-- tables the framework shares with it, keeping every row. Runs once, in one
-- transaction, before the framework's baseline creates the tables missing.

-- Users: the Moodle user ID is the identity's subject, the username its
-- login and the full name its name.
ALTER TABLE users RENAME COLUMN moodle_userid TO subject;
ALTER TABLE users ALTER COLUMN subject TYPE text USING subject::text;
ALTER TABLE users RENAME CONSTRAINT users_moodle_userid_key TO users_subject_key;
ALTER TABLE users RENAME COLUMN username TO login;
ALTER TABLE users ALTER COLUMN login DROP NOT NULL;
ALTER TABLE users RENAME COLUMN fullname TO name;
ALTER TABLE users ALTER COLUMN name DROP NOT NULL;
ALTER TABLE users ADD COLUMN email text;
ALTER TABLE users ADD COLUMN profile jsonb NOT NULL DEFAULT '{}';

-- Sign-in attempts live fifteen minutes; one in flight now has to start over.
DROP TABLE link_attempts;

-- Sessions gain the permissions the user sets: every existing one stays
-- enabled, with every tool, as before.
ALTER TABLE agent_sessions ADD COLUMN enabled boolean NOT NULL DEFAULT true;
ALTER TABLE agent_sessions ADD COLUMN read_only boolean NOT NULL DEFAULT false;
ALTER TABLE agent_sessions ADD COLUMN allowed_tools text[];
ALTER TABLE agent_sessions ADD COLUMN scope jsonb NOT NULL DEFAULT '{}';
ALTER TABLE agent_sessions ADD COLUMN use_outbox boolean NOT NULL DEFAULT false;
ALTER TABLE agent_sessions ADD COLUMN outbox_ttl_minutes integer NOT NULL DEFAULT 60
    CHECK (outbox_ttl_minutes BETWEEN 1 AND 10080);

-- The call log gains the outbox's statuses and columns.
ALTER TABLE tool_calls ADD COLUMN original_arguments jsonb;
ALTER TABLE tool_calls ADD COLUMN outbox_expires_at timestamptz;
ALTER TABLE tool_calls ADD COLUMN decided_at timestamptz;
ALTER TABLE tool_calls DROP CONSTRAINT tool_calls_status_check;
ALTER TABLE tool_calls ADD CONSTRAINT tool_calls_status_check CHECK (status IN
    ('running', 'ok', 'error', 'denied', 'queued', 'discarded', 'expired'));
