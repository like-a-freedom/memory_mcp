# Ornith runtime evidence for HTTP-status diagnostic

The [official Ornith 1.5 35B A3B model card](https://huggingface.co/ornith-ai/Ornith-1.5-35B-A3B) recommends `temperature=0.6`, `top_p=0.95`, and `top_k=20` for general tasks. It describes an agentic/reasoning use case and serving recipes with reasoning and tool-call parsers; it does not certify LM Studio behavior.

LM Studio's [Chat Completions API](https://lmstudio.ai/docs/developer/openai-compat/chat-completions) documents these sampler fields. Its [tool-use documentation](https://lmstudio.ai/docs/developer/openai-compat/tools) warns that calls the parser cannot interpret may be returned as ordinary content instead of structured `tool_calls`.

During the pre-capture audit, LM Studio CLI commit `dcefd5a` reported the alias `memory-local-eval-ornith-v2` loaded and idle, with a 16,384-token context and one parallel request. The server listened on numeric loopback port 1234, `/v1/models` returned HTTP 200, and a short synthetic Chat Completions request returned HTTP 200. A provider-neutral fake-memory smoke then completed one public `assemble_context` tool call in 1.94 seconds. Its content-free receipt records only the pass, count and exact synthetic source-ID match; no model response text was retained.

This establishes current local transport and recall-tool compatibility for this alias. It does not qualify source-authority task quality, establish memory benefit, guarantee the hidden model weights or server build, or explain the earlier `transport` failures. The registered one-shot diagnostic below uses fresh families and the updated status-only stage sidecar to tell whether non-success HTTP statuses occur during that exact workflow.
