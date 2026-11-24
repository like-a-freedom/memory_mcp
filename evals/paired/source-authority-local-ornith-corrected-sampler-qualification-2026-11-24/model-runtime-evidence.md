# Ornith runtime and sampler evidence

Fetched 2026-11-23 through Keenable CLI. The [official Ornith 1.5 35B A3B model card](https://huggingface.co/ornith-ai/Ornith-1.5-35B-A3B) recommends `temperature=0.6`, `top_p=0.95`, and `top_k=20` for general tasks. It describes Ornith as a reasoning model whose serving recipes use a reasoning parser and a tool-call parser to surface OpenAI-compatible tool calls. Its listed serving recipes target recent Transformers, vLLM, and SGLang; they do not certify LM Studio behavior.

The [LM Studio Chat Completions API](https://lmstudio.ai/docs/developer/openai-compat/chat-completions) documents the `temperature`, `top_p`, and `top_k` request parameters. A local loopback preflight against this exact LM Studio model alias returned one schema-valid requested tool call, exposed `reasoning_content`, accepted that assistant turn with the tool result in a follow-up request, and returned the exact expected synthetic value. The receipt records only booleans and counts; raw output was not retained.

This checks sampler request support and the local tool-call/follow-up path. It is not a task-quality result, does not prove the serving implementation matches the model card's vLLM/SGLang parsers, and does not replace the preregistered workflow qualification.
