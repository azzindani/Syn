# Design notes

Background material. These notes record why parts of Syn are shaped the way
they are; they are not instructions for using it, and code comments cite
them by section.

| Note | What it covers |
|---|---|
| [split-plan.md](split-plan.md) | Keeping the tools (usable by any MCP client) separable from Syn's own agent loop, and what goes on which side. Phases 1 and 2 are built: `mcpgate` and the single path through `Runner`. |
| [DIGEST-06-opencode-loop.md](DIGEST-06-opencode-loop.md) | opencode's agent loop: steps, tool calls, compaction, retries. |
| [DIGEST-07-opencode-harness.md](DIGEST-07-opencode-harness.md) | opencode's request side: assembly, per-provider transforms, recovery. |
| [DIGEST-08-t3code-ui.md](DIGEST-08-t3code-ui.md) | t3code's chat UI, the model for the console's structure. |
| [DIGEST-09-t3code-visual.md](DIGEST-09-t3code-visual.md) | t3code's visual layer, the model for the console's look. |

The digests describe other projects (MIT-licensed) as read at a recorded
commit. Ideas were adapted; no code or assets were copied. They are the one
place where other vendors' product and model names are quoted verbatim.
