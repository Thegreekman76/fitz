-- Migration: initial_schema
-- Created: 2026-06-07T13:00:00Z

-- UP
CREATE TABLE "users" (
    "id" bigserial PRIMARY KEY,
    "email" text NOT NULL UNIQUE,
    "password_hash" text NOT NULL DEFAULT '',
    "role" text NOT NULL DEFAULT 'member',
    "created_at" timestamp with time zone NOT NULL
);

CREATE TABLE "projects" (
    "id" bigserial PRIMARY KEY,
    "name" text NOT NULL,
    "description" text NOT NULL DEFAULT '',
    "owner_id" bigint NOT NULL REFERENCES "users"("id") ON DELETE CASCADE,
    "created_at" timestamp with time zone NOT NULL
);

CREATE TABLE "tasks" (
    "id" bigserial PRIMARY KEY,
    "project_id" bigint NOT NULL REFERENCES "projects"("id") ON DELETE CASCADE,
    "title" text NOT NULL,
    "description" text NOT NULL DEFAULT '',
    "status" text NOT NULL DEFAULT 'todo',
    "priority" bigint NOT NULL DEFAULT 3,
    "assignee_id" bigint REFERENCES "users"("id") ON DELETE SET NULL,
    "due_date" date,
    "ai_suggested_priority" bigint,
    "created_at" timestamp with time zone NOT NULL
);

CREATE TABLE "comments" (
    "id" bigserial PRIMARY KEY,
    "task_id" bigint NOT NULL REFERENCES "tasks"("id") ON DELETE CASCADE,
    "user_id" bigint NOT NULL REFERENCES "users"("id") ON DELETE CASCADE,
    "body" text NOT NULL,
    "created_at" timestamp with time zone NOT NULL
);

-- Indexes recomendados para queries comunes.
CREATE INDEX "idx_projects_owner_id" ON "projects" ("owner_id");
CREATE INDEX "idx_tasks_project_id" ON "tasks" ("project_id");
CREATE INDEX "idx_tasks_assignee_id" ON "tasks" ("assignee_id");
CREATE INDEX "idx_comments_task_id" ON "comments" ("task_id");
CREATE INDEX "idx_comments_user_id" ON "comments" ("user_id");

-- DOWN
DROP TABLE "comments";
DROP TABLE "tasks";
DROP TABLE "projects";
DROP TABLE "users";
