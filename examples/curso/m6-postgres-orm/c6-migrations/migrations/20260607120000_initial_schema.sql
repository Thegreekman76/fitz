-- Migration: initial_schema
-- Created: 2026-06-07T12:00:00Z

-- UP
CREATE TABLE "users" (
    "id" bigserial PRIMARY KEY,
    "email" text NOT NULL
);

-- DOWN
DROP TABLE "users";
