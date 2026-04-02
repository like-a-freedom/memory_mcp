-- Migration: Add staging column for online embedding migrations
-- This allows switching embedding providers without database recreation

DEFINE FIELD embedding_next ON TABLE fact TYPE option<array<float>>;
