# AIOS

**An operating system governed by a model. Written from scratch. No Linux.**

> Status: **concept v0.2** — not a single line of code yet, only a specification.
> License: not chosen yet. Language: [Русский](README.md).

---

## The idea in one paragraph

AIOS is a new operating system in which an **LLM is the system's governing core**,
not an application running on top of one. The DeepSeek model understands what the
user wants, plans steps, invokes typed system capabilities (files, services,
network, itself) and reports back. It talks to humans through a console — the way
you work with an agent harness — and to programs through a secured HTTP API. The
kernel is written from scratch: its own ABI, scheduler, file system and network
stack. No Linux clones and no POSIX layer "for compatibility".

## What that means in practice

| Conventional OS | AIOS |
|---|---|
| The administrator knows commands and utilities | The administrator states an intent; the model picks the mechanisms |
| Inference is a process competing with daemons | Inference is a priority class: resources, memory and cores are reserved for the model |
| Programs speak POSIX primitives | Programs speak the API and the capability catalog |
| Rights belong to users and processes | Rights belong to **sessions**: the model proposes, the kernel disposes |
| Audit is a syslog nobody reads | Audit is the foundation: every model action is recorded with its reason and cost |

## Why not Linux

Linux is not "a new system" — it is a system with 30 years of backward
compatibility, where an agent has to pretend to be a process and a model has to
pretend to be a file. AIOS is designed from scratch for the task: the primitives
"session", "capability", "intent", "plan" are first-class. Plus control: millions
of lines of foreign code in the trusted base are pointless if the machine is an
inference appliance.

## Core principles

1. **The model proposes, the kernel disposes.** The model holds no root: every
   action passes a policy engine, a human (when dangerous) and the audit log.
2. **Portability from day one.** One code base, many architectures: x86-64 now,
   aarch64 and RISC-V next, eventually custom silicon. `make image ARCH=…` builds
   the distribution.
3. **All resources belong to the model — on a schedule.** Weights and KV-cache are
   budgeted up front; the system reserve is untouchable.
4. **Closed by default.** Only explicitly opened ports, only an authenticated API,
   only allow-listed capabilities.
5. **A demo at every step.** A milestone without a working demonstration is not a
   milestone.
6. **Memory is part of the system.** Sessions and knowledge survive reboot: transcripts,
   KV-cache snapshots, distilled facts with provenance. Returning to a conversation
   does not re-read the history from scratch.

## What it looks like

The console is a dialogue with the system, not a TTY running bash:

```
aios> why did the httpd service die last night?

  [plan] 1. log.query: events 02:00–06:00, level error
         2. metric.query: memory and CPU for the same window
         3. report to the human, change nothing

  [policy] log.query, metric.query → allowed (read-only)
  [audit]  audit://sess-42/steps/1-2

The service died at 03:14 while allocating 512 MB — the service pool was exhausted
by a background indexing job. The model was idle at the time. Suggestion: cap the
indexer to the background pool — apply? (yes/no)
```

## Long-term memory

The system remembers the state of a conversation, not just the text:

| Kind of memory | What it holds |
|---|---|
| Working | KV-cache of the current generation — in RAM, pinned pages |
| Episodic | session and event transcripts — append-only log on NVMe |
| Semantic | distilled facts and decisions with provenance ("where I know it from, who confirmed it") |
| Procedural | the capability catalog and plan templates |

The argument is the cost of re-reading. Prefill on the flagship model is ≈ 44 tok/s:
"remembering" a 10,000-token conversation costs **≈ 225 seconds** of recomputation. So
a session is parked as a KV-cache snapshot keyed by (model, prefix hash, tokenizer
version); on return the system reads only the tail of the log and pages in what it
needs — seconds of NVMe reads instead of minutes of prefill.

Context is assembled by a deterministic builder with a hard token budget (system
instruction, pinned facts, recent turns, retrieved episodes). The model can ask for
more memory only through the `mem.search` capability — under policy and audit, not by
"reading everything". What the model itself remembers is marked `unverified` until a
human confirms it.

## Hardware and model (MVP contract)

The MVP pins **one board** — writing drivers for every piece of hardware in the
world is pointless:

| Parameter | MVP value |
|---|---|
| Platform | x86-64, UEFI, AMD Ryzen 9 (AM5, Zen 5, AVX-512 VNNI/BF16) |
| Memory | **64 GB DDR5** (2 channels, ≈ 90 GB/s — this is the speed ceiling). Upgrading to 128 GB unlocks V4-Flash |
| Storage | NVMe ≥ 1 TB; layout in [architecture](docs/ARCHITECTURE.md) §5.1.1 |
| Models | on 64 GB: **R1-Distill 32B Q4 (19.9 GB, ~3 tok/s)** and 14B Q4 (9 GB, ~6 tok/s) for the loop, V2-Lite (10.4 GB, 20–25 tok/s). V4-Flash (82–97 GB) does not fit |
| GPU | RTX 3060 12 GB — **unused in the MVP**: CUDA requires a proprietary driver, out of scope |
| Not supported | the 671B family (260–450 GB and 8–12 memory channels), GPU/NPU offload, V4-Flash until a RAM upgrade |

Generation speed is bounded by memory bandwidth, not by the CPU:
`tok/s ≈ 0.63 × BW / weight size` for dense models and `≈ 0.35 × BW / active
params` for MoE. That is what drives model selection.

## Repository layout

```
README.md                      — Russian version
README.en.md                   — this file
docs/SPECIFICATION.md          — specification: concept, requirements, options, roadmap
docs/ARCHITECTURE.md         — architecture: kernel, ABI, memory formats, ADRs, first slice
docs/WORKFLOW.md             — branches, merges, releases
docs/PLAN-M0.md              — M0 task breakdown: infra, boot, memory, formats
docs/research/deepseek-hw-facts.md   — models/hardware: sizes, quantizations, tok/s measurements
docs/research/aios-tech-facts.md     — boot, drivers, stack, effort estimates
```

The personal source note (`spec.md`) never enters the repository — it is listed in
`.gitignore`. The public statement of the idea is the specification only.

## Roadmap

| Milestone | Outcome |
|---|---|
| M0. Metal | kernel boots on real hardware (boot strategy — Q17), memory and output |
| M1. Kernel | SMP, preemptive scheduler, own syscall ABI, processes |
| M2. Storage | NVMe driver, own journaling FS, weight store |
| M3. Network | NIC driver, TCP/IP, DHCP, own HTTP server |
| M4. Inference | own engine, 7–8B, SIMD, KV-cache, OpenAI-compatible streaming |
| M5. Provisioning | first boot: download model, verify signature, raise the API |
| M6. Control plane | capability catalog, policy, audit, console — the model runs the system |
| M7. Portability | aarch64/RISC-V ports, multi-architecture distribution builds |

Detailed exit criteria and estimates live in the
[specification](docs/SPECIFICATION.md).

## Honest status

- This is a **concept**, not a working system. There is no code.
- The path to an MVP demo is estimated at **30–45 person-months**: a team of 4–6
  low-level engineers ≈ 9–12 months, one person — 2.5–4 years. The number comes
  from measurements (800–1300 debugged lines/month, sizes of real drivers), not
  from optimism, and it sits in the specification so it does not surface at the end.
- The architecture is **not fixed**: open questions (Q1–Q23) are listed in §15 of
  the specification. The biggest one is the boot loader: Limine (saves 4–6 PM but
  is third-party) versus our own UEFI loader (fully ours).

## Contributing

It is too early to write code — architecture first. But welcome: reasoned critique
of the specification, benchmarks on target hardware, links to relevant
implementations (bare-metal inference, Rust OSes, MoE on CPU).

## License

Not chosen yet. No code is published before a license is selected. DeepSeek
weights (R1, V4) are MIT; llama.cpp/ggml are MIT.
