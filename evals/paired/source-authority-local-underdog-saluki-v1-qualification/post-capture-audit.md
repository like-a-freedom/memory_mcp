# Underdog Saluki v1 post-capture audit

## Disposition

The registered v1 capture is invalid for model or workflow quality scoring. It
is preserved unchanged as a transport-compatibility failure. Do not replay it,
repair its artifacts, or count its task metrics as evidence about the model.

## Evidence

- All eight arm receipts are `model_failure` after one turn, with zero input
  tokens, zero output tokens, and zero MCP tool calls.
- Every trace ends with the normalized `transport` error class. No task answer
  was generated or assessed.
- A separate synthetic request reproduced HTTP 400 when the request contained
  two consecutive `system` messages. Merging the system instructions into one
  message returned HTTP 200 from the same local model endpoint.
- The model card's recommended direct-tool-call request also produced a tool
  call with a single system message. This checks transport/tool wiring only;
  it does not validate task performance.

## Corrective action

The provider-neutral runner now combines its system instruction and task rules
into one system message. A fake-loopback regression test verifies this request
contract. Optional provider-specific top-level parameters can be supplied from
a JSON file and must be hash-pinned; they cannot override runner-owned protocol
or sampling fields. The new disjoint v2 cohort freezes these changes and omits
the JSON Schema response format because the exact model/server smoke returned
no tool call when that format was enabled.

The exact LM Studio HTTP error body is not retained by the runner, consistent
with its content-free artifact contract. Root cause attribution is instead
supported by the controlled request-shape repro above. No memory implementation
or deterministic memory score changed as a result of this pilot.
