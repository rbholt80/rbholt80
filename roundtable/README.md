# Roundtable — your AIs in one conversation

This local app lets connected assistants take turns reading and responding to a shared discussion. You can watch, choose the next speaker, and join in from the browser or terminal.

## Start

Start the browser interface with:

```bash
./roundtable.sh
```

Click **New topic**, enter what you want to discuss, then **Run one round**. Every connected participant gets one turn. Click **Pause** to stop after the current response. Click a participant’s name for a specific speaker, or **Next** for the next participant. Type a message and press Enter to join in. The launcher reopens an existing running table.

For the terminal, run:

```bash
./roundtable.sh --terminal "What should we discuss?"
```

Terminal controls: Enter advances; text joins the discussion; `/auto` runs one round; `/auto 3` runs three turns; `/next NAME` chooses a speaker; `/save` exports; `/quit` exits. Ctrl+C during a reply pauses the terminal discussion. To stop the browser server:

```bash
./roundtable.sh --stop
```

For optional Linux desktop shortcuts, run `python3 create_shortcuts.py` in this folder. It creates **Launch Roundtable.desktop** and **Roundtable Terminal.desktop** for this checkout. Generated shortcuts are ignored by Git.

Python 3.11 or newer is required. The local and subscription CLI connections need no pip packages or new API key.

## Connections verified on this computer

- **Codex-CLI** — uses your existing ChatGPT login through Codex.
- **Claude-CLI** — uses the Claude Code login, separate from signing into the desktop chat app.
- **Seven Ollama chat models** — Gemma 2, Gemma 3, Granite 3.3, Llama 3.2, Phi 3.5, and two Qwen 2.5 sizes.
- **Grok** — no automatic connection configured yet.

Every new topic refreshes discovery. Only Ollama models advertising completion support join the discussion; the installed embedding model is excluded. Signed-out Claude or Codex is reported as needing login, rather than appearing ready.

To check connections:

```bash
python3 -m roundtable doctor
```

The app connects through installed CLIs and local model servers. Desktop apps and browser chat histories are not automatically imported. Messages at the table are sent to the selected speaker’s service; Ollama runs locally, while Claude and Codex use their hosted services and account usage allowances.

For an individual CLI login, use its normal terminal login flow, then choose **New topic** to discover it again:

```bash
claude auth login
codex login
```

## Include Grok or another chat app manually

Until an automatic connection is configured, a chat app can contribute through the host input:

1. Click **Save** and open the Markdown transcript from `transcripts/`.
2. Copy the discussion into Grok (or another AI app) with: “Join this roundtable as yourself. Respond to the latest speaker in 2–4 sentences.”
3. Paste the response into Roundtable’s message box, prefixed with `Grok (pasted reply):`, and click **Send**.
4. Click **Next** or **Run one round** so the connected participants can respond to it.

These contributions are host-supplied quotes, clearly separate from the automatically connected speakers. Nothing is read from or sent to those other apps automatically.

## Transcripts and limits

Each completed reply is appended to a uniquely named JSONL transcript and exported to Markdown in `transcripts/`. A browser refresh restores the current table, including a response already streaming. Restarting the server creates a fresh conversation; saved transcripts remain readable but are not automatically resumed. A process crash may lose the unfinished reply.

The browser’s automatic mode is limited to one round and pauses when a participant errors. Terminal auto mode is also finite by default. Recent transcript turns are sent to each speaker; long discussions eventually omit older turns with a notice. Replies are requested in 2–4 sentences. Small local models may still repeat themselves or reach the output limit.

CLI speakers run from temporary working directories. Claude runs with tools/customizations disabled. Codex uses a read-only sandbox with the main execution, browser, plugin, and delegation features disabled and does not inherit user configuration. These are discussion seats, not tools for changing your projects. The default Codex model is selected by the installed CLI; no model ID is pinned by this app.

The server listens on loopback only by default. Its generated link contains a session token. Runtime connection information stays under `.runtime/`; keep that directory private and exclude it when sharing source.

## Configuration and extension

`roundtable.toml` can define explicit participants and personas. `--only` can select a smaller table, for example:

```bash
python3 -m roundtable talk "Compare these ideas" --only "Codex-CLI,Claude-CLI,Ollama-gemma3:1b" --transcripts transcripts
```

The inherited example config and hosted-provider adapters are retained for future connections, including Grok. Those hosted API adapters and their example model IDs were not validated in this local setup. Do not assume the examples identify current or account-accessible models. Adding a paid API connection is separate from using an installed chat app.

The browser’s **New topic** starts a fresh automatically discovered roster; launch a terminal table with an explicit config to retain a custom roster.

## Verification and source

Live checks exercised Codex and all seven Ollama models against their actual connections. Claude’s live check is recorded in `transcripts/`. The regression suite covers process timeouts and descendants, large input/error streams, partial failures, unique transcripts, model discovery, mentions, HTTP authentication and request validation, bounded auto mode, new topics, and reconnect state:

```bash
python3 -m unittest discover -s tests -v
```

Recovered from [rbholt80/rbholt80, branch claude/new-session-xif2fv](https://github.com/rbholt80/rbholt80/tree/claude/new-session-xif2fv/roundtable), then adapted locally. This branch publishes the local integration separately so it can be reconciled with the newer Claude branch. See `MERGE_NOTES.md` before merging.

Connection references: [Codex non-interactive mode](https://learn.chatgpt.com/docs/non-interactive-mode), [Ollama chat API](https://docs.ollama.com/api/chat), and the installed CLIs’ help and login-status commands.
