-- Add up migration script here
CREATE TABLE IF NOT EXISTS "sessions" (
    "id" TEXT PRIMARY KEY NOT NULL,
    "expires" DATETIME NOT NULL,
    "created" DATETIME NOT NULL,
    "data" TEXT NOT NULL
);

CREATE INDEX "" ON "sessions" (
	"expires"	ASC
);