CREATE TABLE IF NOT EXISTS ops.tasks (
    id BIGSERIAL PRIMARY KEY,
    task_type TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    target TEXT,
    target_appid INTEGER,
    priority INTEGER NOT NULL DEFAULT 100,
    created_by TEXT NOT NULL DEFAULT 'admin',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    claimed_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS tasks_status_priority_created_idx
ON ops.tasks (status, priority, created_at);

CREATE TABLE IF NOT EXISTS ops.task_runs (
    id BIGSERIAL PRIMARY KEY,
    task_id BIGINT NOT NULL REFERENCES ops.tasks(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    summary TEXT
);

CREATE INDEX IF NOT EXISTS task_runs_task_id_started_idx
ON ops.task_runs (task_id, started_at DESC);

CREATE TABLE IF NOT EXISTS ops.task_failures (
    id BIGSERIAL PRIMARY KEY,
    task_id BIGINT REFERENCES ops.tasks(id) ON DELETE SET NULL,
    stage TEXT NOT NULL,
    target TEXT,
    provider TEXT,
    retryable BOOLEAN NOT NULL DEFAULT FALSE,
    attempt INTEGER NOT NULL DEFAULT 1,
    reason TEXT NOT NULL,
    resolved_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS task_failures_unresolved_created_idx
ON ops.task_failures (resolved_at, created_at DESC);
