# Verified integrated build

See `CODEX_STATUS.md` for the current integration record. The verified source
includes Claude through `9544ed6` and the Codex integration on
`codex/roundtable-local-integration`.

Run `python -m unittest discover -s tests -v` from the package directory using
an environment with the optional SDKs installed for all 33 checks. The server
checks require local loopback sockets. Tests use synthetic providers except the
separately run `doctor --probe`, which returned 9/9 successful responses on
Robert's machine.

Browser verification covered reload, a real local streamed response, an exact
quote challenge, usage display, and a cross-model follow-up. The first 8192-token
local context attempt caused an Ollama OOM failure; reducing the configurable
default to 2048 allowed all seven local models to answer subsequent probes.
This is a connection/integration check, not a benchmark of answer quality or
proof that arbitrary long sessions fit in memory.
