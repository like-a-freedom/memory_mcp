# ADR-0084. Bound public explain citations

**Status** Accepted  
**Date** 2026-11-24

A 16,219-byte escaping-heavy episode produced a roughly 60 kB public
explanation because the source body appeared in several response fields.
This can exhaust an agent's context during a routine verification.

The shared MCP and CLI `explain` adapter caps each successful serialized
tool response at 32,768 bytes. The memory context supplies verbatim
citation windows and sorted provenance metadata. The adapter measures the
complete envelope and reports omitted fields or items through partial
reasons and continuation metadata. Full citation text can be reconstructed
with zero-based Unicode scalar offsets. The item offset and provenance
offset count entries in their own lists.

The public response omits bodies from provenance entries. Both compact and
verbose modes preserve an available complete quote when it fits within the
response policy. This amends the explanation behavior described in
[ADR-0022](0022-compact-response-default-for-llm-consumers.md). Existing inputs still use the
same tool. Agents needing full source text can continue the source from
`source_offset=0`. The byte limit applies to the tool payload before
transport framing.

The public response-size profile measures both modes. A synthetic case
checks exact Unicode reconstruction and applies the same byte cap to every
window. See the [design](../superpowers/specs/2026-11-13-explain-bounded-citation-output-design.md).
