# One — Context Management Architecture

> Deep documentation of the five systems One uses to keep AI context lean, coherent,
> and persistent across long sessions and multiple sessions.

---

## Why context management matters

Every conversation with an LLM runs against a hard token ceiling. In practice, a
productive engineering session generates far more signal than a 200K-token window can
hold — tool outputs, code diffs, error traces, and back-and-forth reasoning all pile up.
Without active management the window silently fills, older context falls off, and the
model starts contradicting itself or repeating already-resolved mistakes.

The naive solution is a sliding window: keep the most recent N tokens and discard the
rest. The problem is that the most useful context is rarely the most recent. The decision
made three hours ago explaining *why* a particular approach was rejected is far more
valuable than the verbatim output of a `cargo build` from five minutes ago.

One applies five distinct systems to solve this. They operate at different timescales and
with different goals, but they compose into a single coherent pipeline.

---

## System 1 — Evergreen: Tiered Session Compression

**Source:** `crates/one-core/src/evergreen.rs`, `crates/one-cli/src/tasks/evergreen.rs`

### The concept

Evergreen implements a three-tier memory hierarchy modelled on the CPU cache analogy:
hot (fast, small, recent), warm (medium, older), cold (slow, huge, durable). The same
insight underpins **MemGPT** ([arXiv:2310.08560](https://arxiv.org/abs/2310.08560)), the
UC Berkeley system that first applied OS-style memory hierarchies to LLMs. MemGPT keeps
recent context in the main context window and evicts older messages to external storage,
generating recursive summaries when it does. One applies the same tiering principle
entirely within session state, without requiring an external database for hot/warm tiers.

The theoretical foundation is **recursive summarisation**: rather than compressing the
full raw conversation, Evergreen compresses hot summaries into warm summaries, and warm
summaries into cold ones. Each pass reduces fidelity but preserves the signal that
constrains future decisions. This is the key insight from
[arXiv:2308.15022](https://arxiv.org/abs/2308.15022): recursive summarisation enables
long-term memory without requiring the full context to fit in a single window.

### How it's applied in One

```
ARCHIVE (warm) │    COMPRESS (hot)     │    WRITE (verbatim)
  2nd-pass AI  │   1st-pass AI         │  last 10 turns, always sent
  summary      │   summaries           │  unmodified
  ─────────────┼───────────────────────┼──────────────────────────────►
  oldest                                                           newest
```

The tier constants are:

| Constant | Value | Meaning |
|----------|-------|---------|
| `WRITE_TIER_TURNS` | 10 | Most recent turns, never compressed |
| `COMPRESS_TIER_MAX_TURNS` | 50 | Turns 11–50 from newest → hot compression eligible |
| `MIN_ELIGIBLE_TO_COMPRESS` | 5 | Don't trigger unless at least 5 turns are eligible |
| `MIN_SPAN_TOKENS_TO_COMPRESS` | 500 | Minimum span size before attempting compression |
| `COMPRESSION_API_COST_ESTIMATE` | 1,000 | Assumed tokens consumed per summarisation call |

Each compression tier produces a structured machine-readable record, not a prose
paragraph. The hot prompt (`HOT_COMPRESS_PROMPT`) demands exactly these labelled
sections: `GOAL`, `STATE`, `DECIDED`, `ARTEFACTS`, `ERRORS`, `OPEN`, `RECALL_GAPS`.
The warm prompt collapses hot summaries into: `SESSION_GOAL`, `APPROACH`,
`STABLE_ARTEFACTS`, `CONSTRAINTS`, `RESOLVED`, `SHARP_EDGES`. This structured approach
means downstream systems (BM25 recall, Chronicle synthesis) can parse and index specific
fields rather than treating summaries as opaque text.

#### The ROI gate

A key design constraint: a compression call itself consumes tokens. If you spend 1,200
tokens calling the model to compress a 900-token span, you've made things worse.
Evergreen's ROI gate prevents this:

```rust
// evergreen.rs:198-200
pub fn roi_gate(span_tokens: u64, summary_tokens: u64, compression_api_cost: u64) -> bool {
    let savings = span_tokens.saturating_sub(summary_tokens);
    savings > compression_api_cost
}
```

The check is conservative on purpose: compression only fires when the savings exceed the
API cost of the compression call *in a single subsequent request*. This means the system
breaks even immediately, rather than amortising cost over several future requests.

### How it compares to Claude's internals

Claude Code's built-in compaction (the `/compact` command and auto-compact trigger) is a
single-pass, all-or-nothing operation: when the context window is nearly full, Claude
summarises the *entire conversation* into one large block. That block is detailed but
flat — there is no tiering, no ROI gate, and no structured field extraction.

Evergreen's advantages:

- **Proactive, not reactive.** Evergreen fires after every turn once the compress tier
  fills. Claude's built-in compact fires at ~95% capacity — by which point signal has
  already been lost from the non-recent tail.
- **Preserves exact strings.** The hot prompt explicitly instructs the model to preserve
  file paths, error messages, and function names verbatim. A flat prose summary tends to
  paraphrase these, introducing subtle bugs when a future agent acts on the recalled
  context.
- **Structured for recall.** Parsed `ParsedSections` fields (`artefacts`, `decided`,
  `sharp_edges`, etc.) are stored separately in the `evergreen_chunks` table, enabling
  targeted BM25 retrieval rather than full-text injection.
- **Reversible.** Original messages are marked `is_evergreen_compressed = 1` in the DB
  but never deleted. A `recall_detail` tool can fetch them. Claude's compact
  irreversibly discards the original turns.

The trade-off is complexity. Evergreen requires a persistent DB schema, a background
task, and structured prompts. Claude's single-pass compact is operationally simpler.

---

## System 2 — Auto-Compact: Token Budget Protection

**Source:** `crates/one-core/src/compact/auto_compact.rs`, `crates/one-core/src/compact/prompt.rs`

### The concept

Auto-compact is the emergency safety net. Where Evergreen is proactive and granular,
auto-compact is reactive and blunt: when the conversation is approaching the hard context
limit, summarise everything and replace it with a structured block.

This is the same mechanism used in Claude Code's built-in compaction, documented at
[platform.claude.com/docs/en/build-with-claude/compaction](https://platform.claude.com/docs/en/build-with-claude/compaction).
The Microsoft Agent Framework addresses the same problem differently (atomic group
removal: drop tool call + result pairs together), but One's approach follows Claude
Code's model — summarise rather than truncate, because truncation loses semantic meaning.

The DEV Community article [How We Extended LLM Conversations by 10x with Intelligent
Context Compaction](https://dev.to/amitksingh1490/how-we-extended-llm-conversations-by-10x-with-intelligent-context-compaction-4h0a)
validates the approach: intelligent compaction rather than sliding windows reliably
extends effective conversation length by ~10× vs. raw context limits.

### How it's applied in One

One's auto-compact is calibrated to the actual model being used:

```
Model                  Context window   Auto-compact triggers at
─────────────────────  ───────────────  ──────────────────────────
claude-opus-4-6 [1m]   1,000,000 tok    967,000 tok  (window - 20k output - 13k buffer)
All other models       200,000 tok      167,000 tok
```

There are four distinct threshold levels:

| Level | Buffer | Behaviour |
|-------|--------|-----------|
| Warning | -20k from threshold | UI indicator appears |
| Auto-compact | -13k from effective window | Summarisation fires automatically |
| Manual blocking limit | -3k from effective window | Hard block on new turns |
| Env override | `CLAUDE_CODE_AUTO_COMPACT_WINDOW`, `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE` | Test/override |

The compaction prompt (`prompt.rs`) is structurally identical to Claude Code's own
compaction prompt: `<analysis>` drafting scratchpad (stripped from output) followed by a
`<summary>` block with 9 sections:

1. Primary Request and Intent
2. Key Technical Concepts
3. Files and Code Sections
4. Errors and fixes
5. Problem Solving
6. All user messages
7. Pending Tasks
8. Current Work
9. Optional Next Step

This is intentional. Claude was trained on this exact format — using it maximises
summary fidelity vs. a custom schema the model has never seen.

A **circuit breaker** stops retrying after 3 consecutive failures
(`MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES = 3`), preventing cascade failures where a
failing compaction call consumes the remaining budget and makes the next call worse.

### How it compares to Claude's internals

One's auto-compact is a deliberate replication of Claude Code's own mechanism, extended
with:

- **Explicit model-aware thresholds.** The 1M-context Opus model gets different budgets
  than 200K models. Claude Code's built-in compact doesn't expose this distinction
  programmatically.
- **Circuit breaker.** Claude Code's built-in compact has no documented failure limit;
  repeated failures during a task can silently degrade the session.
- **Transcript reference.** After compaction, the summary message includes a path to the
  full pre-compaction transcript so the model can `Read` it if specific details are
  needed — rather than guessing from the summary.
- **`suppress_follow_up` mode.** After auto-compact the model is instructed to resume the
  last task directly without acknowledging the summary. This avoids the jarring "I've
  been compacted, shall I continue?" interruption pattern.

---

## System 3 — Palimpsest: Living Document Maintenance

**Source:** `crates/one-cli/src/tasks/palimpsest.rs`

### The concept

A palimpsest is a manuscript that has been written on, partially erased, and written on
again — the traces of earlier writing remain visible through the new. That is exactly
what this system does: documentation files accumulate knowledge from ongoing sessions,
layering new understanding over the old while preserving the document's structure.

The concept connects to two research threads:

1. **Retrieval-augmented generation (RAG) with self-updating context.** Rather than
   querying an external index on every turn, Palimpsest embeds learned context directly
   into files the AI is already reading. When the model reads `CLAUDE.md` or any
   `<!-- one:autodoc -->`-tagged file, it is simultaneously consuming the document and
   triggering an update pass that enriches it with what it just learned.

2. **Write-back to authoritative context.** The Reflexion paper
   ([arXiv:2303.11366](https://arxiv.org/abs/2303.11366)) showed that agents improve by
   writing observations back into a persistent memory buffer. Palimpsest echoes that
   principle at the document level — the model's observations about the codebase are
   written back into the authoritative reference documents — though the mechanism is
   different: Reflexion stores critique text in a scratchpad, Palimpsest updates the
   actual files the agent reads.

### How it's applied in One

Any markdown file can opt in with a single HTML comment:

```markdown
<!-- one:autodoc -->
```

When the AI reads such a file, Palimpsest:

1. Detects the `ToolRequest` for `Read` and records the `call_id → (session_id, file_path)` mapping.
2. On the matching `ToolResult`, checks for the `<!-- one:autodoc -->` marker in the output.
3. If found, acquires a per-file `Mutex` (preventing concurrent torn writes across parallel agents).
4. Snapshots the 20 most recent conversation turns, truncating each to 500 characters.
5. Calls the model with the current file content + recent context, instructing it to update the file.
6. Writes the result back if the model preserved the marker (safety check against accidental removal).

The 20-turn / 500-char truncation is a deliberate budget constraint: Palimpsest uses a
small recent context rather than the full session because documentation updates should
reflect *what was just learned*, not re-derive the entire session history. The full
session is already captured by Evergreen.

### How it compares to Claude's internals

Claude Code has no equivalent mechanism. Its `CLAUDE.md` files are static: a human writes
them, Claude reads them, but Claude never writes back. Context learned during a session
evaporates when the session ends.

Palimpsest turns documentation into a two-way channel. The practical effect is that
`CLAUDE.md` files in One-managed projects gradually accumulate session-derived knowledge:
discovered constraints, resolved ambiguities, confirmed architectural decisions. Each
future session starts with richer context than the last without any manual curation.

The limitation: Palimpsest writes at 0.3 temperature with `max_tokens: 2048`, which
keeps updates conservative. Aggressive or hallucinated updates to authoritative docs
would be worse than no update, so the system intentionally under-reaches.

---

## System 4 — Chronicle: Cross-Session Synthesis

**Source:** `crates/one-cli/src/tasks/chronicle.rs`

### The concept

Evergreen handles context within a session. Chronicle handles context *across* sessions —
the "when I come back tomorrow, I shouldn't have to re-explain the last three weeks"
problem.

This maps directly to the research area of **episodic memory for LLMs**. The distinction
is:

- **Episodic memory**: specific events and experiences (what happened in session 47)
- **Semantic memory**: general knowledge synthesised from experiences (what is durably
  true about this project)

MemGPT ([arXiv:2310.08560](https://arxiv.org/abs/2310.08560)) separates these explicitly,
with archival storage for episodes and a recall database for synthesised knowledge.
**Zep** ([arXiv:2501.13956](https://arxiv.org/abs/2501.13956)) takes this further with a
temporal knowledge graph that tracks how facts evolve over time. **LongMem**
([arXiv:2306.07174](https://arxiv.org/abs/2306.07174)) achieves 65K-token cross-session
memory through a frozen LLM backbone + adaptive side-network architecture.

Chronicle's design is simpler than any of these: it synthesises warm/hot Evergreen chunks
from multiple session DBs into a single cold-tier landmark record of 80–120 words.
Simplicity is a feature — a cold record small enough to fit in every system prompt adds
zero marginal cost to each request.

### How it's applied in One

Chronicle is gated by four conditions before firing:

| Gate | Value | Reason |
|------|-------|--------|
| Toggle | `state.chronicle_enabled` | Can be disabled per-project |
| Time | ≥ 12 hours since last run | Prevent synthesis on every turn-end |
| Volume | ≥ 3 sessions with new Evergreen chunks | Don't synthesise from a single session |
| Lock | `chronicle.lock` file-backed process lock | Prevent concurrent synthesis across One instances |

When gates pass, Chronicle:

1. Reads warm chunks from all session DBs for the active project (falls back to hot if no warm available, capped at 2).
2. Concatenates them with `---` separators.
3. Calls the model with `COLD_COMPRESS_PROMPT`, which demands exactly five fields:
   `PROJECT`, `FINGERPRINT`, `KEY_ARTEFACTS`, `SHARP_EDGES`, `RECALL_NOTE`.
4. Saves the result to `~/.one/chronicle.db` (shared across all sessions).
5. Updates the active session's `evergreen_context` so the cold record is injected into the current system prompt immediately.

The cold tier prompt explicitly prioritises "X was rejected because Y" over describing X.
This reflects a key insight: the most valuable cross-session signal is not *what was
built* but *why alternatives were ruled out*. A future agent that doesn't know an
approach was rejected will waste time re-evaluating it.

### How it compares to Claude's internals

Claude Code has no cross-session memory. Each session starts from scratch — the only
persistence is what the user manually maintains in `CLAUDE.md` files.

OpenAI's ChatGPT memory (launched 2024, enhanced January 2026) is the closest commercial
equivalent: it saves memories across sessions and can reference conversations up to a
year old. The difference is that ChatGPT's memory is opaque and user-visible only through
a curated UI. Chronicle's cold records are stored in a local SQLite file, are fully
inspectable, and are explicitly scoped to the project (not the user's entire ChatGPT
history).

Zep's temporal knowledge graph approach is more powerful — it tracks entity relationships
and how they evolve over time. Chronicle's flat 80–120 word cold record loses that
relational richness. The trade-off is operational simplicity: no graph database, no
embedding index, no external API. A cold record is a string in a table.

---

## System 5 — Context Injection & BM25 Recall

**Source:** `crates/one-core/src/evergreen.rs:562-649`, `crates/one-core/src/query_engine.rs:1731-1745`

### The concept

Having a rich memory store is only useful if the right memories surface at the right time.
Injecting the entire Evergreen history into every system prompt would defeat the purpose
of compression. The challenge is *selective recall*: given the current user query, which
stored chunks are relevant?

The standard modern answer is dense vector retrieval: embed both the query and each chunk,
compute cosine similarity, return top-k. One uses **BM25** (Okapi BM25) instead, for
several practical reasons grounded in the retrieval literature:

- **Exact-term precision.** BM25 is a sparse TF-IDF-weighted ranking function. It excels
  at matching exact tokens — file paths, function names, error strings, variable names.
  These are precisely the terms in Evergreen's `ARTEFACTS` sections.
  [Benchmark studies](https://arxiv.org/html/2604.01733) show BM25 outperforms even
  `text-embedding-3-large` on domains with precise technical terminology.
- **No embedding infrastructure.** Dense retrieval requires an embedding model,
  a vector store, and GPU access (or embedding API calls). BM25 runs in microseconds
  in-process with no dependencies.
- **Transparent scoring.** The BM25 score is interpretable: it rewards rare terms that
  appear frequently in a document but rarely across the corpus. This is auditable;
  embedding similarity is not.

The research consensus ([LanceDB hybrid retrieval](https://www.lancedb.com/blog/hybrid-search-combining-bm25-and-semantic-search-for-better-results-with-lan-1358038fe7e6),
[Towards AI](https://towardsai.net/p/artificial-intelligence/enhance-your-llm-agents-with-bm25-lightweight-retrieval-that-works))
is that hybrid BM25 + dense achieves 15–30% better recall than either alone, and up to
13.3% hallucination reduction. One applies BM25 alone rather than hybrid — the
simplification is justified because the cold/warm tiers are *always* injected regardless
of BM25 score, so the ranking only filters hot chunks. The marginal gain from dense
embedding on that already-narrow slice doesn't justify the dependency.

### How it's applied in One

`build_recall_context` in `evergreen.rs` implements a two-pass selection strategy:

**Pass 1 — Deterministic inclusion:**
- Always include cold and warm tier chunks (unconditional — these are orientation-level context).
- Always include any hot chunk whose `artefacts` field contains a term from the current query (exact substring match, case-insensitive).

**Pass 2 — BM25 ranking:**
- Collect remaining hot chunks not already included.
- Run Okapi BM25 across their summaries with the current query.
- Include the top 3 by score.

The BM25 implementation is hand-rolled in Rust with no external crates (lines 330–416):

```rust
// Tuned BM25 parameters
const K1: f64 = 1.2;   // Term frequency saturation
const B: f64 = 0.75;   // Document length normalisation
```

Standard BM25 parameters (`k1=1.2, b=0.75`) are used — these are well-validated defaults
across the IR literature and appropriate for the short summary documents being ranked.

The selected chunks are prefixed with `RECALL_PREAMBLE`, which explicitly calibrates the
model's confidence in different tiers:

> - Treat ARTEFACTS as reliable (exact names/paths were preserved)
> - Treat DECIDED as reliable but verify reversals with the user if stakes are high
> - Treat OPEN as stale — these may have been resolved since compression
> - If RECALL_GAPS are listed, acknowledge them rather than guessing

This calibration prevents the model from treating compressed summaries as ground truth.
The explicit instruction to acknowledge `RECALL_GAPS` rather than infer them is
particularly important: models tend to confabulate when context is incomplete. Naming the
gap is better than silently filling it.

### How it compares to Claude's internals

Claude Code does not perform any selective recall from prior context. Its system prompt
is static per session: `CLAUDE.md` files, tool definitions, and project instructions.
Nothing from a previous turn is ranked or selected — either it's in the current context
window, or it's gone.

The BM25 recall layer in One provides something qualitatively different: *relevance-gated
memory injection*. The system prompt grows richer with the session's history, but only
with the parts that are relevant to the current task. A turn about database schema
optimisation doesn't pollute the system prompt during a CSS debugging session.

The limitation vs. a full hybrid retrieval system (like Zep's temporal knowledge graph)
is that BM25 cannot handle semantic similarity — a query about "latency" won't match a
chunk that only mentions "response time". For engineering sessions where the vocabulary
is precise and consistent, this is rarely a problem. For more conversational or
ambiguous queries, hybrid retrieval would outperform.

---

## How the systems compose

```
User submits message
        │
        ▼
┌──────────────────────────┐
│  Auto-compact check      │  If tokens ≥ threshold → summarise entire conversation,
│  (before API call)       │  replace with structured summary block
└─────────────┬────────────┘
              │
              ▼
┌──────────────────────────┐
│  Build system prompt     │  Load evergreen_context (BM25-filtered recall)
│  + recall injection      │  Inject cold/warm unconditionally; BM25-rank hot chunks
└─────────────┬────────────┘
              │
              ▼
┌──────────────────────────┐
│  API call to provider    │
└─────────────┬────────────┘
              │
              ▼
┌──────────────────────────┐
│  Persist to SessionDb    │  Messages marked is_evergreen_compressed = 0
└─────────────┬────────────┘
              │
        ┌─────┴─────┐
        │           │
        ▼           ▼
┌─────────────┐  ┌──────────────┐
│  Evergreen  │  │  Palimpsest  │  If any autodoc file was read this turn,
│  background │  │  background  │  update it with recent 20-turn context
│  task       │  │  task        │
└──────┬──────┘  └──────────────┘
       │
       │  If uncompressed_count > thresholds:
       │  1. Plan compression (classify turns into tiers)
       │  2. ROI gate (savings > 1,000 tokens?)
       │  3. Call model → structured summary
       │  4. Save to evergreen_chunks, mark messages compressed
       │  5. Rebuild evergreen_context
       │  6. Emit EvergreenCompressed event
       │
       ▼
┌──────────────────────────┐
│  Chronicle background    │  If time gate (≥12h) and volume gate (≥3 sessions):
│  task                    │  Synthesise warm/hot chunks → 80-120 word cold record
└──────────────────────────┘
```

---

## Comparative summary

| System | Timescale | Analogue in prior art | One's distinguishing choice |
|--------|-----------|----------------------|----------------------------|
| **Evergreen** | Within session | MemGPT hot/warm tiers, recursive summarisation | ROI gate; structured fields preserved verbatim; reversible |
| **Auto-Compact** | Near context limit | Claude Code auto-compact | Model-aware thresholds; circuit breaker; transcript reference |
| **Palimpsest** | After each turn | Reflexion-style self-improvement | Triggered by actual file reads; per-file mutex; marker-gated |
| **Chronicle** | Across sessions | OpenAI memory, Zep episodic store | Local SQLite; project-scoped; 80–120 word hard limit |
| **BM25 Recall** | Each request | Hybrid RAG retrieval | Tier-aware (cold/warm unconditional); artefact matching first; no embedding infra |

### Key design principles running through all five

1. **Exact strings over paraphrase.** Every compression prompt instructs the model to
   preserve file paths, function names, error messages, and version numbers verbatim.
   Paraphrased technical context causes downstream errors.

2. **Conservative cost accounting.** The ROI gate, circuit breaker, per-tier word limits,
   and Chronicle time/volume gates all reflect the same principle: a bad context update
   is worse than no update.

3. **Reversibility.** Compressed messages are marked, not deleted. Original content
   persists in the SessionDb and is retrievable via `recall_detail`.

4. **Graceful degradation.** Each system has an explicit toggle (`evergreen_enabled`,
   `chronicle_enabled`, `palimpsest_enabled`). When a system is disabled or fails, the
   rest continue operating.

5. **No cloud dependency.** All five systems operate entirely locally. Memory is in
   `~/.one/`, not in Anthropic's servers, OpenAI's memory store, or a third-party
   service.

---

## Verdict: how much better than Claude's internals?

The honest answer depends on session length and workflow type.

**For short, one-shot sessions (< 30 minutes, single task):** no measurable difference.
Auto-Compact is the only system that activates meaningfully, and it is a refinement of
Claude Code's own mechanism rather than a replacement. The gap here is narrow.

**For long sessions (1–4 hours, multi-file refactors, debugging chains):** Evergreen
provides a qualitative improvement. Claude Code's built-in compact fires once, at ~95%
capacity, replacing everything with a flat prose block. Evergreen fires continuously,
preserving structured fields with exact strings and keeping the write tier verbatim. The
practical effect is that the model in a late-session turn has access to structured
`DECIDED`/`ARTEFACTS`/`SHARP_EDGES` records from earlier in the session, rather than a
dense prose paragraph it must parse. The ROI gate also means Evergreen spends compression
budget only where it saves more than it costs.

**For multi-session workflows (days to weeks on a single project):** Chronicle and
Palimpsest close a gap that Claude Code simply does not address. Claude Code has no
cross-session memory and no mechanism for documentation to accumulate session knowledge.
A One-managed project after 10 sessions has cold-tier landmark records in
`~/.one/chronicle.db` and progressively richer `<!-- one:autodoc -->` files. The model
starts each session oriented to the project's current state rather than cold. This is the
largest capability gap between One and Claude Code's native context handling.

**The BM25 recall layer** prevents that accumulated context from becoming noise: the
system prompt doesn't grow unboundedly, it selects the chunks that are relevant to the
current query. Claude Code has no equivalent — its system prompt is static per session.

In aggregate: One's context management is roughly equivalent to Claude Code's for
throwaway sessions, meaningfully better for long sessions, and categorically better for
persistent project work. The cost is operational complexity — five background tasks,
a per-project SQLite schema, and structured prompts — versus Claude Code's single
reactive compact.
