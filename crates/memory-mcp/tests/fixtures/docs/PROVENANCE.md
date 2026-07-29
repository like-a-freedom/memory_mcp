# Fixture provenance

These binary fixtures are vendored from upstream open-source test/sample repositories so document-ingest evals run against real-world containers instead of hand-rolled placeholders.

## Vendored upstream fixtures

- `sample.pdf`
  - Source: `https://raw.githubusercontent.com/J-F-Liu/lopdf/main/assets/example.pdf`
  - Upstream repo: `https://github.com/J-F-Liu/lopdf/tree/main/assets`
  - License: MIT (`https://raw.githubusercontent.com/J-F-Liu/lopdf/master/LICENSE`)
  - Stable marker observed locally: `Hello World`

- `sample.docx`
  - Source: `https://raw.githubusercontent.com/apache/poi/trunk/test-data/document/SampleDoc.docx`
  - Upstream repo: `https://github.com/apache/poi/tree/trunk/test-data/document`
  - License family: Apache License 2.0
  - Stable markers observed locally: `I am a test document`, `This is page 1`

- `sample.xlsx`
  - Source: `https://raw.githubusercontent.com/apache/poi/trunk/test-data/spreadsheet/SampleSS.xlsx`
  - Upstream repo: `https://github.com/apache/poi/tree/trunk/test-data/spreadsheet`
  - License family: Apache License 2.0
  - Stable markers observed locally: `Test spreadsheet`, `2nd row`, `Start of 2nd sheet`

- `sample.pptx`
  - Source: `https://raw.githubusercontent.com/apache/poi/trunk/test-data/slideshow/SampleShow.pptx`
  - Upstream repo: `https://github.com/apache/poi/tree/trunk/test-data/slideshow`
  - License family: Apache License 2.0
  - Stable markers observed locally: `Title of the first slide`, `This is the second slide`

## Local text fixtures

- `sample.md` — local markdown fixture for deterministic plain-text ingest.
- `sample.eml` — local RFC822-style email fixture for deterministic mail ingest.
- `sample.html` — local HTML fixture served from a loopback test server for deterministic URL ingest.

Last refreshed: 2026-04-07.
