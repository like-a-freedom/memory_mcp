# C3 v1 fixture review

**Reviewed:** 2026-11-25, before opening any v2 capture.

The v1 capture is preserved as executed, but its correction scenario is not a
valid product-defect reproduction. Its structured records put free prose after
the `value:` field, separated by `;`. In the product parser,
`explicit_schema_fields` treats semicolons as record separators; each segment
must be a recognized `key:value` field. An unrecognized prose segment returns
an empty field map. That discards the explicit subject/dimension/value schema.

The captured `explain` evidence confirms the consequence: the 14-event history
resolved into truncated shared names such as `North Canal Gauge`, generated
19 provenance results, and returned five items for a query about one gauge.
The conflict oracle then failed on `decisionunobservable`, `forbiddensource`,
and `incorrectrole`. Those results are evidence of a malformed evaluation
fixture and entity-link contamination, not evidence that the product mishandles
a valid structured correction.

Do not change or delete the frozen v1 corpus, registration, or receipts. They
remain useful as a regression test for fixture review and as a captured example
of why source IDs alone do not validate a semantic oracle. The valid v2 family
uses complete explicit fields (`subject`, `measure`, numeric `value`, `unit`)
and disjoint entity names. Its registration is frozen independently before its
first capture.
