# Ornith runtime evidence for clarification-trace diagnostic

The official [Ornith 1.5 35B A3B model card](https://huggingface.co/ornith-ai/Ornith-1.5-35B-A3B) recommends `temperature=0.6`, `top_p=0.95`, and `top_k=20` for general tasks. It describes agentic/reasoning use cases but does not certify this local runtime.

LM Studio's [Chat Completions API](https://lmstudio.ai/docs/developer/openai-compat/chat-completions) documents these sampler fields. Its [tool-use documentation](https://lmstudio.ai/docs/developer/openai-compat/tools) warns that calls the parser cannot interpret may be returned as ordinary content instead of structured `tool_calls`.

At the 2026-11-24 pre-capture check, LM Studio CLI commit `dcefd5a` reported `memory-local-eval-ornith-v2` loaded and idle with a 16,384-token context and one parallel request. The server listened on numeric loopback port 1234 and `/v1/models` returned HTTP 200. The provider-neutral fake-memory smoke completed one public `assemble_context` tool call in 1.95 seconds. Its content-free receipt records only pass, count and the exact synthetic source-ID match; no model response text was retained.

This establishes local transport and tool-call compatibility only. It does not qualify source-authority task quality, establish memory benefit, fingerprint hidden model weights or server build, or prove the cause of the earlier partial capture. This one-shot development diagnostic uses disjoint synthetic families after a deterministic clarification-counter/trace fix; it is not powered confirmation.
