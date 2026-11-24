# Local recall diagnostic screen

Apple Silicon, loopback Chat Completions, `liquid/lfm2.5-1.2b`. Both
modes ran three synthetic cases once per arm on the same memory
binary, with one tool call, three turns, 384 output tokens and 25 seconds
per trial. The case manifest, fixed requests, plans and normalized receipts
are archived alongside this report. Each plan pins the binary used for
its run. Regex extraction and disabled embeddings are part of the stdio
fixture profile.

| Mode | Trials | Successful MCP reads | Reads with required source | Historical replies at registered cutoff | Passing answers | Input / output tokens |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Live | 6 | 2 | 0 | 0/2 | 0 | 6418 / 482 |
| Fixed reply | 6 | 2 | 2 | 2/2 | 0 | 7438 / 528 |

Both modes had zero clarification turns and zero budget exits.
The ordinary case produced a non-JSON final response without reading in
both arms. The contradiction case recorded a model transport failure
before any read, so the agent run supplied no claim-status observation.
A separate public-tool preflight found both registered sources in that case
with no typed claim status under the fixture's extraction profile.

For the historical case, the agent attempted the wrong snapshot in both
modes. Live returned an empty, partial reply. Fixed reply returned the
required source at the registered revision and valid time, with partial
coverage of historical channels. The model still produced a non-JSON final
response. There was no verifiable citation in either mode.

**Decision.** This screen supports investigating query formation and final
response handling separately. It leaves the earlier NO-GO in force. The
historical contrast justifies a task-focused reproduction with a model that
can emit the final schema. The transport failure must be resolved before
using the contradiction case to assess model behavior. This screen provides
no basis for changing memory retrieval or promoting a product candidate.

## Qwen follow-up before A8

The same historical case and an ordinary positive control were run locally
with Qwen3.8-27B MLX, 4,096 context tokens, 512 output tokens, one tool call,
three turns and 90 seconds per trial. Both modes and both arms retrieved the
ordinary source and exited on the registered budget before a verifiable
answer. Both historical pairs recorded a model transport failure before
reading. The completed live run used the preserved incumbent executable
after the binary fingerprint guard interrupted a run during a separate
product build.

The Qwen follow-up supplied no historical reply for a presentation
comparison. The previous LFM screen also had an unresolved output-contract
failure with adequate fixed historical evidence. Keep A8 at NO-GO and leave
the public response format unchanged.
