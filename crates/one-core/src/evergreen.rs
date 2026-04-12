//! Evergreen tiered context compressor — pure planning logic.
//!
//! The Evergreen system keeps the AI context window lean across long sessions
//! by organising conversation history into three tiers:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │ ARCHIVE  │     COMPRESS     │         WRITE (verbatim)      │
//! │ 2nd-pass │   1st-pass AI   │   last WRITE_TIER_TURNS turns  │
//! │ summary  │    summaries    │   always sent verbatim         │
//! └─────────────────────────────────────────────────────────────┘
//!   oldest ◄─────────────────────────────────────────► newest
//! ```
//!
//! This module is **pure logic** — no async, no DB, no IO.
//! The background task in `one-cli` owns the DB calls and AI summarization calls.

// ── Constants ─────────────────────────────────────────────────────────────────

/// Number of most-recent turns always kept verbatim in context (write tier).
pub const WRITE_TIER_TURNS: usize = 10;

/// Total uncompressed turns (write + compress) before archive tier kicks in.
/// Turns older than this become 2nd-pass (archive) candidates.
pub const COMPRESS_TIER_MAX_TURNS: usize = 50;

/// Don't trigger a compression pass unless at least this many turns are eligible.
/// Avoids wasting API calls on tiny spans.
pub const MIN_ELIGIBLE_TO_COMPRESS: usize = 5;

/// Minimum tokens a span must contain before compression is attempted.
/// Compressing a 200-token span costs more than it saves.
pub const MIN_SPAN_TOKENS_TO_COMPRESS: u64 = 500;

/// Estimated tokens consumed per AI summarization call (input + output overhead).
/// Used by the ROI gate to decide whether a pass is worthwhile.
pub const COMPRESSION_API_COST_ESTIMATE: u64 = 1_000;

// ── Config ────────────────────────────────────────────────────────────────────

/// Configurable Evergreen thresholds.
/// Defaults mirror the module-level constants.
#[derive(Debug, Clone)]
pub struct EvergreenConfig {
    /// Turns to always keep verbatim (write tier).
    pub write_tier_turns: usize,
    /// Total uncompressed-turn budget; beyond this → archive tier.
    pub compress_tier_max_turns: usize,
    /// Minimum eligible turns required to trigger a pass.
    pub min_eligible: usize,
    /// Minimum token count in a span before compression is tried.
    pub min_span_tokens: u64,
    /// Estimated tokens spent per AI compression call (for ROI gate).
    pub compression_api_cost: u64,
}

impl Default for EvergreenConfig {
    fn default() -> Self {
        Self {
            write_tier_turns: WRITE_TIER_TURNS,
            compress_tier_max_turns: COMPRESS_TIER_MAX_TURNS,
            min_eligible: MIN_ELIGIBLE_TO_COMPRESS,
            min_span_tokens: MIN_SPAN_TOKENS_TO_COMPRESS,
            compression_api_cost: COMPRESSION_API_COST_ESTIMATE,
        }
    }
}

// ── Tier classification ───────────────────────────────────────────────────────

/// Which compression tier a turn falls into, given current totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierKind {
    /// Always sent verbatim — no compression.
    WriteVerbatim,
    /// Eligible for 1st-pass AI summarization.
    CompressFirst,
    /// Already 1st-pass compressed; eligible for 2nd-pass (archive) summarization.
    ArchiveSecond,
}

/// Classify a single turn by its 0-indexed position *from the oldest turn*.
///
/// `total` — total number of uncompressed turns currently in the session.
pub fn classify_turn(position_from_oldest: usize, total: usize, cfg: &EvergreenConfig) -> TierKind {
    let position_from_newest = total.saturating_sub(1 + position_from_oldest);

    if position_from_newest < cfg.write_tier_turns {
        TierKind::WriteVerbatim
    } else if position_from_newest < cfg.compress_tier_max_turns {
        TierKind::CompressFirst
    } else {
        TierKind::ArchiveSecond
    }
}

// ── Compression plan ──────────────────────────────────────────────────────────

/// A contiguous span of turns to be compressed in a single AI call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressBatch {
    /// 0-indexed position of the first turn in this batch (from oldest).
    pub start_idx: usize,
    /// 0-indexed position of the last turn in this batch (from oldest), inclusive.
    pub end_idx: usize,
    /// Compression tier to apply.
    pub kind: TierKind,
}

impl CompressBatch {
    pub fn len(&self) -> usize {
        self.end_idx.saturating_sub(self.start_idx) + 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The complete compression plan for one pass.
#[derive(Debug, Clone)]
pub struct EvergreenPlan {
    /// 1st-pass batch (if any).  Spans the compress tier.
    pub compress_batch: Option<CompressBatch>,
    /// 2nd-pass batch (if any).  Spans the archive tier.
    pub archive_batch: Option<CompressBatch>,
}

impl EvergreenPlan {
    pub fn is_empty(&self) -> bool {
        self.compress_batch.is_none() && self.archive_batch.is_none()
    }
}

/// Compute the compression plan for `uncompressed_count` turns.
///
/// Returns `None` when no compression is needed yet.
/// Returns an `EvergreenPlan` (possibly with only one batch populated) otherwise.
///
/// The plan describes *positions* within the current uncompressed-turn list,
/// not message IDs. The caller (Phase 4b background task) maps positions to
/// actual DB row IDs.
pub fn plan_compression(uncompressed_count: usize, cfg: &EvergreenConfig) -> Option<EvergreenPlan> {
    // Nothing to do if we're within the write tier
    let eligible = uncompressed_count.saturating_sub(cfg.write_tier_turns);
    if eligible < cfg.min_eligible {
        return None;
    }

    // ── Archive (2nd-pass) span ──────────────────────────────────────────────
    // Turns older than compress_tier_max_turns from the newest are archive candidates.
    let archive_batch = if uncompressed_count > cfg.compress_tier_max_turns {
        let archive_count = uncompressed_count - cfg.compress_tier_max_turns;
        Some(CompressBatch {
            start_idx: 0,
            end_idx: archive_count - 1,
            kind: TierKind::ArchiveSecond,
        })
    } else {
        None
    };

    // ── Compress (1st-pass) span ─────────────────────────────────────────────
    // Between archive and write tiers.
    let compress_start = archive_batch.as_ref().map(|b| b.end_idx + 1).unwrap_or(0);
    let compress_end = uncompressed_count.saturating_sub(cfg.write_tier_turns + 1);

    let compress_batch = if compress_end >= compress_start {
        Some(CompressBatch {
            start_idx: compress_start,
            end_idx: compress_end,
            kind: TierKind::CompressFirst,
        })
    } else {
        None
    };

    let plan = EvergreenPlan {
        compress_batch,
        archive_batch,
    };

    if plan.is_empty() { None } else { Some(plan) }
}

// ── ROI gate ──────────────────────────────────────────────────────────────────

/// Returns `true` when compressing `span_tokens` down to `summary_tokens` is
/// worthwhile relative to the API cost of the summarization call.
///
/// Decision rule:
///   `tokens_saved = span_tokens - summary_tokens`
///   Compress only when `tokens_saved > compression_api_cost`
///
/// This ensures that a compression call breaks even within **one** subsequent
/// request — a conservative but safe default.  The background task should
/// also apply the `min_span_tokens` check before calling this.
pub fn roi_gate(span_tokens: u64, summary_tokens: u64, compression_api_cost: u64) -> bool {
    let savings = span_tokens.saturating_sub(summary_tokens);
    savings > compression_api_cost
}

/// Rough token estimate: 1 token ≈ 4 UTF-8 bytes.
/// Used when exact token counts are unavailable.
pub fn estimate_tokens(content: &str) -> u64 {
    (content.len() as u64).div_ceil(4)
}

// ── Section parser ────────────────────────────────────────────────────────────

/// Parsed fields extracted from a structured evergreen summary.
/// Fields not present in the output are `None` / empty.
#[derive(Debug, Clone, Default)]
pub struct ParsedSections {
    /// GOAL / SESSION_GOAL / PROJECT
    pub goal: Option<String>,
    /// STATE (hot only)
    pub state: Option<String>,
    /// APPROACH (warm only)
    pub approach: Option<String>,
    /// FINGERPRINT (cold only)
    pub fingerprint: Option<String>,
    /// ARTEFACTS / STABLE_ARTEFACTS / KEY_ARTEFACTS bullet list
    pub artefacts: Vec<String>,
    /// ERRORS bullet list
    pub errors: Vec<String>,
    /// OPEN bullet list
    pub open_items: Vec<String>,
    /// DECIDED bullet list
    pub decided: Vec<String>,
    /// CONSTRAINTS bullet list
    pub constraints: Vec<String>,
    /// SHARP_EDGES bullet list
    pub sharp_edges: Vec<String>,
    /// RECALL_GAPS / RECALL_NOTE text
    pub recall_note: Option<String>,
    /// RESOLVED text
    pub resolved: Option<String>,
}

/// Parse a structured evergreen summary into discrete fields.
///
/// Looks for ALL_CAPS section headers at the start of a line (e.g. `GOAL:`)
/// and extracts either the inline value or subsequent bullet list.
pub fn parse_sections(text: &str) -> ParsedSections {
    let mut sections = ParsedSections::default();

    // Split text into (header, body) pairs.
    // A header is a line matching /^[A-Z_]+:/.
    let mut pairs: Vec<(&str, String)> = Vec::new();
    let mut current_header: Option<&str> = None;
    let mut current_body = String::new();

    for line in text.lines() {
        let trimmed = line.trim();
        // Detect "WORD_WORD:" at start of line
        let is_header = trimmed
            .split_once(':')
            .map(|(key, _)| {
                !key.is_empty() && key.chars().all(|c| c.is_ascii_uppercase() || c == '_')
            })
            .unwrap_or(false);

        if is_header {
            if let Some(h) = current_header {
                pairs.push((h, current_body.trim().to_string()));
            }
            let (key, rest) = trimmed.split_once(':').unwrap();
            current_header = Some(key);
            current_body = rest.trim().to_string();
        } else if current_header.is_some() {
            if !current_body.is_empty() {
                current_body.push('\n');
            }
            current_body.push_str(line);
        }
    }
    if let Some(h) = current_header {
        pairs.push((h, current_body.trim().to_string()));
    }

    // Map parsed pairs to fields.
    for (header, body) in pairs {
        match header {
            "GOAL" | "SESSION_GOAL" | "PROJECT" => {
                sections.goal = Some(body);
            }
            "STATE" => {
                sections.state = Some(body);
            }
            "APPROACH" | "FINGERPRINT" => {
                if header == "APPROACH" {
                    sections.approach = Some(body.clone());
                } else {
                    sections.fingerprint = Some(body.clone());
                }
            }
            "ARTEFACTS" | "STABLE_ARTEFACTS" | "KEY_ARTEFACTS" => {
                sections.artefacts = extract_bullets(&body);
            }
            "ERRORS" => {
                sections.errors = extract_bullets(&body);
            }
            "OPEN" => {
                sections.open_items = extract_bullets(&body);
            }
            "DECIDED" => {
                sections.decided = extract_bullets(&body);
            }
            "CONSTRAINTS" => {
                sections.constraints = extract_bullets(&body);
            }
            "SHARP_EDGES" => {
                sections.sharp_edges = extract_bullets(&body);
            }
            "RECALL_GAPS" | "RECALL_NOTE" => {
                let bullets = extract_bullets(&body);
                sections.recall_note = if bullets.is_empty() {
                    Some(body)
                } else {
                    Some(bullets.join("; "))
                };
            }
            "RESOLVED" => {
                sections.resolved = Some(body);
            }
            _ => {}
        }
    }

    sections
}

fn extract_bullets(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim())
        .filter(|l| l.starts_with("- ") || l.starts_with("* "))
        .map(|l| l[2..].trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

// ── BM25 retrieval ────────────────────────────────────────────────────────────

/// Tokenise text for BM25: lowercase, split on non-alphanumeric, drop short tokens.
fn tokenise(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|t| t.to_lowercase())
        .filter(|t| t.len() > 2)
        .collect()
}

/// BM25 relevance score for `query` against a single `document`.
/// Uses corpus-level term frequencies from `all_docs`.
/// k1=1.2, b=0.75 (standard defaults).
fn bm25_score(
    query_tokens: &[String],
    doc_tokens: &[String],
    avg_dl: f64,
    n: usize,
    df: &std::collections::HashMap<String, usize>,
) -> f64 {
    const K1: f64 = 1.2;
    const B: f64 = 0.75;

    let dl = doc_tokens.len() as f64;
    let norm = 1.0 - B + B * dl / avg_dl.max(1.0);

    let mut tf_map = std::collections::HashMap::<&str, usize>::new();
    for t in doc_tokens {
        *tf_map.entry(t.as_str()).or_insert(0) += 1;
    }

    query_tokens
        .iter()
        .map(|term| {
            let tf = *tf_map.get(term.as_str()).unwrap_or(&0) as f64;
            let df_t = *df.get(term).unwrap_or(&0) as f64;
            let idf = ((n as f64 - df_t + 0.5) / (df_t + 0.5)).max(0.0).ln_1p();
            let tf_norm = tf * (K1 + 1.0) / (tf + K1 * norm);
            idf * tf_norm
        })
        .sum()
}

/// Rank chunk indices by BM25 relevance to `query`.
/// Returns indices sorted descending by score (most relevant first).
pub fn rank_by_relevance<'a>(query: &str, summaries: &'a [&'a str]) -> Vec<(usize, f64)> {
    if summaries.is_empty() {
        return Vec::new();
    }

    let query_tokens = tokenise(query);
    let doc_tokens: Vec<Vec<String>> = summaries.iter().map(|s| tokenise(s)).collect();

    let avg_dl = doc_tokens.iter().map(|d| d.len() as f64).sum::<f64>() / doc_tokens.len() as f64;

    // Build document-frequency map
    let mut df = std::collections::HashMap::<String, usize>::new();
    for doc in &doc_tokens {
        let unique: std::collections::HashSet<&str> = doc.iter().map(|s| s.as_str()).collect();
        for term in unique {
            *df.entry(term.to_string()).or_insert(0) += 1;
        }
    }

    let n = summaries.len();
    let mut scored: Vec<(usize, f64)> = doc_tokens
        .iter()
        .enumerate()
        .map(|(i, doc)| (i, bm25_score(&query_tokens, doc, avg_dl, n, &df)))
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
}

/// Check whether `query` mentions any of the known artefacts.
/// Returns matched artefact strings (for use in retrieval).
pub fn match_artefacts<'a>(query: &str, artefacts: &[&'a str]) -> Vec<&'a str> {
    let q = query.to_lowercase();
    artefacts
        .iter()
        .copied()
        .filter(|a| {
            let name = std::path::Path::new(a)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(a);
            q.contains(&a.to_lowercase()) || q.contains(&name.to_lowercase())
        })
        .collect()
}

// ── Prompts ───────────────────────────────────────────────────────────────────

/// Compression prompt for the **hot** tier (first-pass, recent turns).
/// Produces a 300–500 word structured machine-readable record.
pub const HOT_COMPRESS_PROMPT: &str = "\
You are a lossless context compressor for an engineering session. Your output will be \
the sole source of truth for a future agent that has no memory of this conversation.

Produce a structured summary using EXACTLY these labelled sections. Be maximally \
dense — this is not a narrative, it is a machine-readable record.

GOAL: [one sentence: what we are trying to build, fix, or decide]

STATE: [current working state — what is building/passing/broken right now]

DECIDED:
- [decision made] — instead of [rejected alternative] — because [reason]
(repeat for each decision; always include the tradeoff)

ARTEFACTS:
- [exact file paths, function/method names, env var names, table/schema names, \
API endpoints, port numbers, package names and versions]
(if none: ARTEFACTS: none yet)

ERRORS:
- [exact error message or type] → [how resolved] | OPEN if unresolved
(include the stack frame or line if mentioned)

OPEN:
- [unresolved questions, known unknowns, next intended actions]

RECALL_GAPS:
- [anything you suspect was discussed but is not captured in the excerpt — \
e.g. \"earlier auth flow details not in this excerpt\"]

Rules:
- No prose paragraphs. Dense labelled bullets only.
- Preserve exact strings: file paths, error messages, function names. Do not paraphrase these.
- If something was decided and then reversed, record the reversal with reason.
- Omit: greetings, affirmations, explanations of what Claude is doing, filler.
- 300–500 words total. If you are under 300, you have lost signal. \
If you are over 500, you have added noise.

CONVERSATION:
";

/// Compression prompt for the **warm** tier (second-pass, compressing hot summaries).
/// Produces a 150–250 word session arc.
pub const WARM_COMPRESS_PROMPT: &str = "\
You are compressing a hot-tier context summary into a warm-tier session arc. \
The input is a previous hot-tier summary, not raw conversation.

Preserve only what a future agent needs to understand the shape of this session \
without re-reading it. Collapse what is resolved. Keep what constrains future decisions.

Output format:

SESSION_GOAL: [one sentence]

APPROACH: [chosen approach + why alternatives were rejected — \
this is the most important field]

STABLE_ARTEFACTS:
- [file paths, schema names, contracts that are now settled and unlikely to change]

CONSTRAINTS:
- [discovered constraints: API limits, env requirements, platform quirks, team \
decisions, anything that will bite a future agent if forgotten]

RESOLVED: [brief list of threads that are fully closed — one line each]

SHARP_EDGES:
- [known gotchas, partial implementations, things that look done but aren't]

Rules:
- 150–250 words. Hard limits.
- Do not re-expand resolved errors unless the resolution itself is a constraint.
- If the hot summary contained RECALL_GAPS, propagate them here.

HOT_SUMMARY:
";

/// Compression prompt for the **cold** tier (cross-session landmark, 80–120 words).
/// Input is one or more warm-tier session arcs from different sessions.
pub const COLD_COMPRESS_PROMPT: &str = "\
Compress these warm-tier summaries into a single cold-tier landmark record. \
A future agent will use this to orient at the start of a session where most details are lost.

Output format — exactly these five fields, nothing else:

PROJECT: [name / one-line description]
FINGERPRINT: [the core technical approach in ≤2 sentences — why this approach, not what it does]
KEY_ARTEFACTS: [3–6 most critical file/schema/endpoint names, comma-separated]
SHARP_EDGES: [1–3 gotchas a fresh agent must know before touching anything here]
RECALL_NOTE: [one sentence on what is known to be missing or stale]

Rules:
- 80–120 words total across all five fields. Hard limit.
- Every word earns its place.
- Prefer \"X was rejected because Y\" over describing X.
- If approaches conflict across sessions, record the most recent.

WARM_SUMMARIES:
";

/// Recall preamble injected at the top of the system prompt when evergreen
/// chunks are available. `{blocks}` is replaced with the formatted tier blocks.
pub const RECALL_PREAMBLE: &str = "\
# Evergreen Context

You have access to compressed context from previous work in this session via an \
evergreen memory store. This context was produced by a tiered summarisation pipeline \
and may be incomplete.

Before acting on recalled context:
- Treat ARTEFACTS as reliable (exact names/paths were preserved)
- Treat DECIDED as reliable but verify reversals with the user if stakes are high
- Treat OPEN as stale — these may have been resolved since compression
- If RECALL_GAPS are listed, acknowledge them if relevant rather than guessing
- Warm-tier records are orientation only — do not over-index on them

If asked about something not in the compressed record, say so explicitly rather \
than inferring.

[CONTEXT FOLLOWS]
";

/// Build the recall context string from a list of `(tier, summary)` pairs
/// (ordered oldest-first). Returns `None` if `chunks` is empty.
/// A single chunk ready for recall injection.
pub struct RecallChunk<'a> {
    pub tier: &'a str,
    pub summary: &'a str,
    /// Artefacts extracted from the structured fields.
    pub artefacts: &'a [String],
}

/// Build the recall context string, optionally filtered and ranked by `query`.
///
/// When `query` is supplied:
/// 1. Any chunk whose artefacts overlap with the query is always included.
/// 2. Remaining chunks are BM25-ranked; only the top-3 by relevance are included.
/// 3. Cold/warm tiers are always included regardless of score.
///
/// When `query` is `None`, all chunks are included in tier order.
pub fn build_recall_context(chunks: &[RecallChunk<'_>], query: Option<&str>) -> Option<String> {
    if chunks.is_empty() {
        return None;
    }

    let selected: Vec<&RecallChunk<'_>> = if let Some(q) = query {
        // Collect all artefact strings for matching.
        let all_artefacts: Vec<&str> = chunks
            .iter()
            .flat_map(|c| c.artefacts.iter().map(|a| a.as_str()))
            .collect();
        let matched_artefacts = match_artefacts(q, &all_artefacts);

        let mut include = vec![false; chunks.len()];

        // Always include cold/warm and any chunk with a matching artefact.
        for (i, chunk) in chunks.iter().enumerate() {
            if chunk.tier != "hot" {
                include[i] = true;
            }
            if chunk.artefacts.iter().any(|a| {
                matched_artefacts
                    .iter()
                    .any(|m| m.eq_ignore_ascii_case(a.as_str()))
            }) {
                include[i] = true;
            }
        }

        // BM25-rank the remaining hot chunks; include top-3.
        let hot_indices: Vec<usize> = chunks
            .iter()
            .enumerate()
            .filter(|(i, c)| c.tier == "hot" && !include[*i])
            .map(|(i, _)| i)
            .collect();

        if !hot_indices.is_empty() {
            let summaries: Vec<&str> = hot_indices.iter().map(|&i| chunks[i].summary).collect();
            let ranked = rank_by_relevance(q, &summaries);
            for (local_idx, _score) in ranked.into_iter().take(3) {
                include[hot_indices[local_idx]] = true;
            }
        }

        chunks
            .iter()
            .enumerate()
            .filter(|(i, _)| include[*i])
            .map(|(_, c)| c)
            .collect()
    } else {
        chunks.iter().collect()
    };

    if selected.is_empty() {
        return None;
    }

    let mut out = RECALL_PREAMBLE.to_string();
    for chunk in selected {
        let label = match chunk.tier {
            "warm" => "WARM — session arc",
            "cold" => "COLD — landmark",
            _ => "HOT — recent context",
        };
        out.push_str(&format!("\n--- {label} ---\n{}\n", chunk.summary));
    }
    Some(out)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> EvergreenConfig {
        EvergreenConfig::default()
    }

    // ── classify_turn ────────────────────────────────────────────────────────

    #[test]
    fn write_tier_is_newest_turns() {
        let total = 20;
        let cfg = cfg();
        // The 10 newest turns (positions 10..19 from oldest) are write tier.
        for i in 10..20 {
            assert_eq!(classify_turn(i, total, &cfg), TierKind::WriteVerbatim);
        }
    }

    #[test]
    fn compress_tier_is_middle_turns() {
        let total = 30;
        let cfg = cfg();
        // Positions 0..19 are eligible; 10..19 are compress, 0..9 are also compress.
        // With total=30, compress_tier_max=50 → no archive tier.
        for i in 0..20 {
            assert_eq!(classify_turn(i, total, &cfg), TierKind::CompressFirst);
        }
    }

    #[test]
    fn archive_tier_kicks_in_beyond_compress_max() {
        let total = 60;
        let cfg = cfg();
        // Positions 0..9 (oldest) are archive (total - compress_tier_max = 10).
        for i in 0..10 {
            assert_eq!(classify_turn(i, total, &cfg), TierKind::ArchiveSecond);
        }
        // Positions 10..49 are compress.
        for i in 10..50 {
            assert_eq!(classify_turn(i, total, &cfg), TierKind::CompressFirst);
        }
        // Positions 50..59 are write.
        for i in 50..60 {
            assert_eq!(classify_turn(i, total, &cfg), TierKind::WriteVerbatim);
        }
    }

    // ── plan_compression ─────────────────────────────────────────────────────

    #[test]
    fn no_plan_within_write_tier() {
        assert!(plan_compression(5, &cfg()).is_none());
        assert!(plan_compression(10, &cfg()).is_none());
        // 10 + min_eligible - 1 = 14 → still below threshold
        assert!(plan_compression(14, &cfg()).is_none());
    }

    #[test]
    fn plan_has_only_compress_batch_for_small_sessions() {
        // 15 turns: 5 eligible, exactly at min_eligible → should plan
        let plan = plan_compression(15, &cfg()).unwrap();
        assert!(plan.archive_batch.is_none());
        let cb = plan.compress_batch.unwrap();
        assert_eq!(cb.kind, TierKind::CompressFirst);
        assert_eq!(cb.start_idx, 0);
        // compress_end = 15 - (10 + 1) = 4
        assert_eq!(cb.end_idx, 4);
        assert_eq!(cb.len(), 5);
    }

    #[test]
    fn plan_has_both_batches_for_large_sessions() {
        // 60 turns: archive = positions 0..9, compress = 10..49, write = 50..59
        let plan = plan_compression(60, &cfg()).unwrap();

        let ab = plan.archive_batch.unwrap();
        assert_eq!(ab.kind, TierKind::ArchiveSecond);
        assert_eq!(ab.start_idx, 0);
        assert_eq!(ab.end_idx, 9);

        let cb = plan.compress_batch.unwrap();
        assert_eq!(cb.kind, TierKind::CompressFirst);
        assert_eq!(cb.start_idx, 10);
        // compress_end = 60 - 11 = 49
        assert_eq!(cb.end_idx, 49);
    }

    // ── roi_gate ─────────────────────────────────────────────────────────────

    #[test]
    fn roi_positive_when_savings_exceed_cost() {
        // 10_000 tokens compressed to 500 → saves 9_500 > 1_000 cost → compress
        assert!(roi_gate(10_000, 500, 1_000));
    }

    #[test]
    fn roi_negative_when_savings_below_cost() {
        // 1_200 → 500, saves 700 < 1_000 → skip
        assert!(!roi_gate(1_200, 500, 1_000));
    }

    #[test]
    fn roi_negative_when_summary_larger_than_span() {
        // Pathological: summary inflated — savings = 0 → skip
        assert!(!roi_gate(500, 600, 1_000));
    }

    // ── estimate_tokens ──────────────────────────────────────────────────────

    #[test]
    fn estimate_tokens_rounds_up() {
        assert_eq!(estimate_tokens("abcd"), 1); // 4 bytes → 1 token
        assert_eq!(estimate_tokens("abcde"), 2); // 5 bytes → 2 tokens (rounded up)
        assert_eq!(estimate_tokens(""), 0);
    }
}
