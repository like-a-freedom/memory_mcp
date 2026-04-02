-- Store entity links as either string IDs or typed record references.
-- This preserves backward compatibility with existing string-based entity_links
-- while allowing new writes to use typed record<entity> references.
-- The Rust deserializer in src/models.rs handles both forms transparently.
DEFINE FIELD OVERWRITE entity_links ON fact TYPE option<array<string | record<entity>>> DEFAULT [];
