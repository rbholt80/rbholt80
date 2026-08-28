# warrant

**A model may propose. Only fixed, auditable code may decide.**

This is the second half of that sentence, as a library.

It sits between something that proposes actions — a language model, an agent, a
script — and the code that carries them out, and it does four things:

1. Makes the request say what it is: `fs.delete:/home/robert/old.txt`.
2. Grades it against a table **you** wrote, not one it inferred.
3. Answers allow / confirm / deny / never from ordered rules, citing the exact
   line that decided.
4. Writes down how to reverse the action **before** letting it happen, in an
   append-only log you can read with `jq`.

It performs no actions and reverses none. It has no dependencies. It is about
two thousand lines of Rust, and the point is that you can read all of them.

```
$ warrant check agent:claude fs.delete:/home/robert/thesis.pdf
fs.delete:/home/robert/thesis.pdf: needs confirmation — this cannot be taken back [policy.warrant:7]
$ echo $?
10
```

## Why not just ask the model to be careful

Because then the request and the judgment come from the same place.

A model that has been talked into deleting your home directory has also been
talked into believing that deleting it is fine, and it will explain why,
fluently, at length. Every safety property worth having has to hold *even when
the thing asking is confidently wrong* — which means it cannot be the thing
asking that checks.

So the model gets to say what it wants, in a form a rule can be applied to, and
something with no opinions applies the rule.

## The two files

**`grades.warrant`** — what your capabilities cost if the request was wrong.
Warrant knows nothing about `fs` or `db` or `mail`; they are your strings, and
this is where you say what they mean.

```
read      fs.read fs.list fs.stat
write     fs.write fs.move
elevated  fs.delete pkg.install net.connect
critical  sys.firmware db.drop
```

**`policy.warrant`** — who may do what. Ordered, first match wins, so narrow
exceptions go above broad defaults.

```
# decision  subject   capability              # reason
never       *         fs.read:/**/.ssh/**     # private keys stay on the disk
never       *         fs.write:/boot/**

deny        agent:*   pkg.install             # a person installs software
confirm     agent:*   fs.delete:~/**          # say it out loud first
allow       agent:*   fs.read:~/**
allow       agent:*   fs.write:~/**
allow       user      fs.*:~/**
```

`never` is not a louder `deny`. A `deny` is an ordered rule, and a rule above it
can win. A `never` is a floor: checked before the ordered rules, unliftable by
anything below it, by any agent, or by a human clicking yes. It is for the
things that stay true however good the argument for the exception sounds —
which, when the argument is being written by a language model, is the category
that matters.

## Four defaults, and why

| Default | Why |
|---|---|
| An **ungraded** capability is `critical` | A capability nobody thought about is exactly the one to be asked about. The opposite default is wrong precisely once. |
| An **unmatched** request is denied | No rule permitting it means nobody considered it. |
| An `allow` **cannot lift an agent** above `write` | Policy files are written once; capabilities are added forever. A broad `allow agent:* fs.*` written when `fs` meant reading must not silently authorise a `fs.wipe` added later. |
| A `never` **cannot be confirmed past** | See above. |

The first two compose into something useful: add a capability, forget to grade
it, and a broad `allow` still will not hand it to an agent — it lands above the
ceiling and becomes a confirmation.

## Undo is written before the action, not after

Every action produces two lines in the journal: one before it runs, one after.

That looks like overhead until you consider the case this exists for: **the
process dies during the action.** Journalling once, afterwards, means the crash
that most needs an undo record is the one that has none.

So the reversal is written and flushed to disk before your code is allowed to
act, and an entry with no completion is a durable record of something that may
be half-done. `warrant unfinished` lists them.

```json
{"kind":"act","seq":41,"ts":1756339200,"subject":"agent:claude","capability":"fs.write:/home/robert/notes.md","risk":"write","decision":"allow","matched":"policy.warrant:8","intent":"save the draft","undo":{"note":"restore notes.md","data":{"backup":"/var/b/7"}}}
{"kind":"end","seq":41,"ts":1756339201,"outcome":"ok","detail":"wrote 412 bytes"}
```

One JSON object per line. `tail -f`, `grep`, `jq`. An audit log that needs its
own tooling does not get audited.

Warrant stores the reversal; it never performs one. `data` is whatever *you*
need — a backup path, an inverse API call, a transaction id. Hand it back with
`take_undo(seq)`, do the work, and call `reverted(seq)` **only once it actually
worked**: a failed undo must not leave the journal claiming the action was taken
back.

## Rust

```rust
let guard = Guard::open(Path::new("/var/lib/myapp"), policy, grades)?;

let cap = Capability::parse("fs.write:/home/robert/notes.md")?;
let ruling = guard.rule(&Subject::Agent("claude".into()), &cap);

if !ruling.allowed() {
    println!("{}", ruling.explain());
    guard.refuse(&ruling, "asked to overwrite notes")?;
    return Ok(());
}

// The undo reaches the disk here, before anything is touched.
let pending = guard.begin(&ruling, "save the draft",
    Undo::new("restore notes.md", json_obj([("backup", "/var/b/7".into())])))?;

fs::write(&path, &text)?;

pending.finish(Outcome::Ok, &format!("wrote {} bytes", text.len()))?;
```

The ordering is not a convention you have to remember. There is no way to get a
`Pending` without the undo having been written, and no way to close a record
without a `Pending`.

## Python, Kotlin, anything else

The binary speaks line-delimited JSON on stdin and stdout — one request object
per line, one response per line, in order. A subprocess and two pipes, which
works the same from every language and has to be compiled against nothing.

`bindings/warrant.py` is a standard-library client (no build step):

```python
from warrant import Warrant, Refused, NeedsConfirmation

w = Warrant(dir="~/.local/share/myapp")

with w.act("agent:claude", "fs.write:/home/robert/notes.md",
           intent="save the draft",
           undo=("restore notes.md", {"backup": "/var/b/7"})) as a:
    path.write_text(text)
    a.detail = "wrote %d bytes" % len(text)
```

A refused action raises before the body runs. A body that raises is journalled
`failed` — not `refused` — because it was authorised and it ran, so it may have
half-happened. That is the case the undo was written down in advance for.

`NeedsConfirmation` is a distinct exception from `Refused`, so "nobody asked"
can never be mistaken for "somebody said yes". Pass `confirmed_by="robert"` once
one has.

## From a shell script or a tool hook

`warrant check` answers with its exit code, so nothing has to parse anything:

| Code | Meaning |
|---|---|
| 0 | allow |
| 10 | confirm — ask a person |
| 20 | deny |
| 30 | never — do not offer to ask |
| 64 | malformed request |

```sh
if warrant check "agent:$AGENT" "$CAP" >/dev/null; then
    do_the_thing
fi
```

## Install

```sh
cargo install --path .          # or: cargo build --release
warrant init ~/.local/share/myapp
```

No dependencies, so it builds on a machine with nothing but a Rust toolchain,
and there is no supply chain between an inference result and your filesystem.

```sh
cargo test          # 72 tests
cd bindings && WARRANT_BIN=../target/release/warrant python3 -m unittest test_warrant
```

## Where this came from

Extracted from the broker in [NOUS OS](../nous-os), where it was welded to one
particular set of filesystem capabilities. The generalisation is that the risk
table and the protected paths became *data the host supplies* rather than
tables compiled into the crate — which is what lets it guard a database, an API
client or a mail sender as readily as a filesystem.

## Licence

MIT OR Apache-2.0.
