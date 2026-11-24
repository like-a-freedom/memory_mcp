# Underdog Saluki local qualification v2

A fresh disjoint four-family source-authority workflow qualification for the local Underdog Saluki model. This cohort follows the v1 adapter compatibility failure and is independently frozen. The v1 transport-failed capture remains immutable and contributes no task-quality evidence.

The cohort uses a single combined system message (runner contract), JSON-schema response format disabled (the exact-model smoke did not emit a tool call when structured output was enabled), temperature 0, top_p 0.95, top_k 20, and the pinned provider parameter `chat_template_kwargs.enable_thinking=false`. These settings were checked on a synthetic loopback request before preregistration; this compatibility check is not model-quality evidence.

Four fresh synthetic families cover two conversational formation cases and two operator-import cases. Both arms use the same product binary; only the workflow differs. The qualification gate remains task-oracle success at least 3/4 per arm, independent probes 4/4 per arm, candidate file tasks at least 1/2, operator imports/extractions 2/2, and zero hard violations or budget overruns. Failure is a NO-GO for this exact configuration, not a memory-treatment estimate.

No retries, tuning, or replacement of run-01 are allowed after the cohort is exposed. Offline artifact comparison is required after capture.
