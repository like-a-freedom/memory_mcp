-- Add edge origin classification for extracted vs inferred graph links.

DEFINE FIELD OVERWRITE origin ON edge TYPE string DEFAULT 'extracted';

UPDATE edge SET origin = 'extracted' WHERE origin IS NONE;