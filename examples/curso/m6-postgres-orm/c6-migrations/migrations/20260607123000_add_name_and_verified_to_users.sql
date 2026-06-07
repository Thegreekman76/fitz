-- Migration: add_name_and_verified_to_users
-- Created: 2026-06-07T12:30:00Z

-- UP
ALTER TABLE "users" ADD COLUMN "name" text NOT NULL DEFAULT '';
ALTER TABLE "users" ADD COLUMN "email_verified" boolean NOT NULL DEFAULT false;

-- DOWN
ALTER TABLE "users" DROP COLUMN "email_verified";
ALTER TABLE "users" DROP COLUMN "name";
