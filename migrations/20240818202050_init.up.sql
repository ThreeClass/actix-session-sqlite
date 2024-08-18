-- Add up migration script here
CREATE TABLE IF NOT EXISTS "sessions" (
    "id" bigint PRIMARY KEY not null,
      "expires" DATETIME NOT NULL,
      "created" DATETIME NOT NULL,
      "data" text NOT NULL
);