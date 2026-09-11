# Connection verification

Nine participants returned real responses: Codex-CLI, Claude-CLI, and these Ollama models:

- gemma2:2b
- gemma3:1b
- granite3.3:2b
- llama3.2:1b
- phi3.5:3.8b
- qwen2.5:0.5b-instruct
- qwen2.5:1.5b-instruct

The browser server also completed a shared discussion between Claude, Codex, and Gemma. Its SSE endpoint returned the full nine-participant roster and four saved turns, and the Markdown export succeeded. No participants reported errors during these live checks.

Eight regression tests passed, covering large CLI streams, process-tree timeouts, partial failures, unique transcripts and mentions, local discovery without SDKs, HTTP authentication and malformed requests, one-round automatic mode, new topics, and partial-reply reconnect state.

Browser functionality was checked through HTTP/SSE endpoints and served-page inspection; no visual browser automation was performed.

Grok is not automatically connected. Hosted API adapters, Gemini CLI, LLM CLI, and LM Studio were not live-tested.
