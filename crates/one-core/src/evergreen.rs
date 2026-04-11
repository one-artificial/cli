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
