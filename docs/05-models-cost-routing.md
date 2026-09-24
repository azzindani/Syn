# 05 — Models, Providers, Cost Routing

## Names (from thread)
- OpenAI: `ChatGPT desktop app` (macOS+Windows, now Chat+Work+Codex; old = `ChatGPT Classic`). Download chatgpt.com/download.
- Anthropic: `Claude Desktop`.

## In-app model change: yes, both
- Claude Desktop: picker or `/model`, several first-party tiers, some paid-only. Mid-session, history stays.
- ChatGPT desktop: model + reasoning effort under the composer, several first-party tiers. Shared `~/.codex/config.toml: model="<id>"`.

## External providers (OpenRouter/DeepSeek): yes via hack, no 1-click
- Claude Desktop Gateway mode -> `https://openrouter.ai/api`, bearer `sk-or-v1...`, static key. Shows gateway models. Undocumented, fragile.
- ChatGPT desktop Codex part -> `~/.codex/config.toml [model_providers.openrouter] base_url="https://openrouter.ai/api/v1" env_key="OPENROUTER_API_KEY" wire_api="responses"`, `model=<any ID>`; DeepSeek via `[model_providers.deepseek]`+models.json. Needs env visible to Dock app (launchctl/setx), restart; picker not provider-aware; chat stays OpenAI-only.
- Clean multi-provider UX today = Opencode/LibreChat. the agent differentiator = native OpenRouter selector in desktop AND Office add-ins with shared history.

## Computer-use cost (surveyed Sep 2026)
- The best first-party computer-use model at the time: ~1M ctx, 128K out. Short: $10 in / $1 cached / $12.50 write / $50 out. Long (>272K): $20/$2/$25/$75. Batch ~half, fast mode ~double, plus a computer-use per-call fee.
- Perf: 72.6% OSWorld 2.0, the top of its field. Still dozens of image-token loops per task.
- Route: small/standard slots for routine work, coding slot for code, reasoning slot at low/medium effort as the vision fallback only. Cost per successful task, not per token.
