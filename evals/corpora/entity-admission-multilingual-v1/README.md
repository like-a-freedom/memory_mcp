# Entity admission, independent source-labelled development corpus v1

Labels frozen before current extractor predictions on 27 November 2026.
Single author review of synthetic source text; not independent human annotation
or a blind confirmation corpus. No benchmark/model output supplied the labels.

Entities are durable named identities, not numeric amounts, currencies, physical
units or metric dimensions. Entity labels are exhaustive per source; omissions
and extras both count. Named organizations/people/projects remain identities
when they contain digits or have numerical attributes. Language coverage is
finite: English, Russian, French, German. Case class and language are separately
reported; totals cannot hide missing non-English names.

The `manual` row is explicit source evidence of identity; an additional real
manual-resolve control verifies caller registration independent of automatic NER.
Precision/recall/F1 and linking use actual public extraction outcomes. Historical
automatic projections remain in the older report; no facts are removed or silently
reprocessed. Unfixed findings remain visible rather than adjusting labels.
