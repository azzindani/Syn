# Harness 05 — Models, Providers, Cost Routing

## Names (from thread)
- OpenAI: `ChatGPT desktop app` (macOS+Windows, now Chat+Work+Codex; old = `ChatGPT Classic`). Download chatgpt.com/download.
- Anthropic: `Claude Desktop`.

## In-app model change: yes, both
- Claude: picker or `/model`, Haiku 4.5 / Sonnet 5 / Opus 4.8 / Fable 5. Free Haiku+Sonnet, paid +Opus/Fable. Mid-session, history stays.
- ChatGPT desktop: model+reasoning under composer. GPT-5.6 Sol/Terra/Luna + Power/Smarter/Faster + Low->Ultra/Max. Shared `~/.codex/config.toml: model="gpt-5.6"`.

## External providers (OpenRouter/DeepSeek): yes via hack, no 1-click
- Claude Desktop Gateway mode -> `https://openrouter.ai/api`, bearer `sk-or-v1...`, static key. Shows gateway models. Undocumented, fragile.
- ChatGPT desktop Codex part -> `~/.codex/config.toml [model_providers.openrouter] base_url="https://openrouter.ai/api/v1" env_key="OPENROUTER_API_KEY" wire_api="responses"`, `model=<any ID>`; DeepSeek via `[model_providers.deepseek]`+models.json. Needs env visible to Dock app (launchctl/setx), restart; picker not provider-aware; chat stays OpenAI-only.
- Clean multi-provider UX today = Opencode/LibreChat. Harness differentiator = native OpenRouter selector in desktop AND Office add-ins with shared history.

## Astra cost (verified Sep 2026)
- `gpt-6-astra`, 1.05M ctx, 128K out. Short: $10 in / $1 cached / $12.50 write / $50 out. Long (>272K): $20/$2/$25/$75. Batch/Flex ~half, Fast ~double + computer-use per-call fee.
- Perf: 72.6% OSWorld 2.0 vs 65.7% Sol, ~47% faster (40 vs 75 min), 1.9x Mind2Web. Still dozens of image-token loops.
- Route: Luna/Terra routine, Sol code, Astra low/medium fallback. Cost per successful task, not per token.
