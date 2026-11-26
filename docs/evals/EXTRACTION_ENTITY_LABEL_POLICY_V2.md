# Extraction entity labels, revision 2

This source review is independent of the extractor's current predictions. Prior diagnostic predictions are already known from the earlier audit, so this is not a blinded annotation exercise. The ruling below is based on what the original text denotes, rather than choosing labels that match observed output.

An automatic entity is a named referent (person, organization, product, project/topic or place) that can carry identity across statements. A quantity, date, percentage, measurement unit or metric dimension is information about a referent, not evidence of a new identity. Explicit manual resolution remains authoritative: this automatic admission rule must not prevent a caller from intentionally naming a concept or a scalar-looking identifier.

Source decisions for the original extraction corpus:

- `Subject: Atlas follow-up` names the subject of the email. Together with the action items, `Atlas` is a named project/topic referent. The source does not establish that it is a person or company, but omitting the name from a complete entity expectation is an annotation defect. Revision 2 adds `Atlas` to ext-009 only.
- `ARR grew to $3M` uses ARR as a measure and `$3M` as its value. Neither is an identity. `Alice Smith reports ARR is $7M` and `Bob Jones reports MRR is $140K` name people; ARR/MRR remain metric dimensions and the amounts remain quantities. The named people stay expected, while the measure/value tokens stay excluded.
- `Carla Gomez says the marketing budget is $3M` names Carla Gomez. The budget and amount are attributes/values, not additional named entities.
- Names containing digits are not quantities merely because they contain digits. A company such as `3M` or `Studio 54` remains eligible when its context supports organization identity. A named acronym such as `NASA` remains eligible in identity context. No lexical blacklist of ARR, MRR, money strings, company names or metric names is authorized by this policy.

`tests/fixtures/extraction_cases.json` remains the historical revision-1 fixture. `tests/fixtures/extraction_cases_v2.json` is a new reviewed development corpus whose only label change is the source-justified Atlas addition. Expected entity and warning lists remain closed-world; warnings and source content are unchanged. Revision-1 raw artifacts remain historical evidence and must not be rewritten to the new labels.

Production regressions must exercise the public extraction boundary: automatic scalar/metric noise is rejected, named referents survive, and explicit manual resolve remains available. Native before/after results must use the same revision-2 corpus so the production change is separated from the annotation correction.
