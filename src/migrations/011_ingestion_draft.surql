-- Migration: 011_ingestion_draft.surql
-- APP-03: Draft tables for human-in-the-loop ingestion review

DEFINE TABLE draft_ingestion SCHEMAFULL;
DEFINE FIELD draft_id ON draft_ingestion TYPE string;
DEFINE FIELD scope ON draft_ingestion TYPE string;
DEFINE FIELD status ON draft_ingestion TYPE string;
DEFINE FIELD created_at ON draft_ingestion TYPE datetime;
DEFINE FIELD expires_at ON draft_ingestion TYPE datetime;
DEFINE FIELD access_ctx ON draft_ingestion TYPE object FLEXIBLE;

DEFINE TABLE draft_item SCHEMAFULL;
DEFINE FIELD draft_id ON draft_item TYPE string;
DEFINE FIELD item_id ON draft_item TYPE string;
DEFINE FIELD item_type ON draft_item TYPE string;
DEFINE FIELD status ON draft_item TYPE string;
DEFINE FIELD payload ON draft_item TYPE object FLEXIBLE;
DEFINE FIELD original_payload ON draft_item TYPE option<object> FLEXIBLE;
DEFINE FIELD confidence ON draft_item TYPE number;
DEFINE FIELD rationale ON draft_item TYPE option<string>;
DEFINE FIELD source_snippet ON draft_item TYPE option<string>;

DEFINE INDEX draft_ingestion_draft_id ON draft_ingestion COLUMNS draft_id UNIQUE;
DEFINE INDEX draft_item_draft_id ON draft_item COLUMNS draft_id;
DEFINE INDEX draft_item_item_id ON draft_item COLUMNS item_id;
