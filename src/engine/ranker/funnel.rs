pub const H_VECTOR: f64 = 0.20;
pub const H_AUTHORITY: f64 = 0.08;
pub const H_AX: f64 = 0.12;

/// Diversity penalty factor per prior occurrence of the same domain.
pub const DIVERSITY_PENALTY: f64 = 0.85;

/// Half-life for freshness decay in days.
pub const FRESHNESS_HALF_LIFE_DAYS: f64 = 180.0;

/// Stage 1 Funnel Cutoff Thresholds:
pub const FUNNEL_SEMANTIC_THRESHOLD: f64 = 0.60;
pub const FUNNEL_QUALITY_THRESHOLD: f64 = 0.45;
pub const MAX_STACK_CANDIDATES: usize = 128;

/// Compact descriptor representing a candidate result in stack memory without heap allocation.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CandidateDescriptor {
    pub index: usize,
    pub vector_sim: f32,
    pub quality_metric: f32,
    pub bm25_score: f32,
    pub freshness_score: f32,
    pub graph_score: f32,
    pub example_count: u8,
    pub final_rank_score: f32,
    pub passed_gates: bool,
}

/// Zero-Heap Candidate Funnel Engine for high-throughput L1/L2 cache ranking.
pub struct ZeroHeapFunnelEngine {
    pub buffer: [CandidateDescriptor; MAX_STACK_CANDIDATES],
    pub count: usize,
    pub semantic_threshold: f32,
    pub quality_threshold: f32,
}

impl ZeroHeapFunnelEngine {
    pub fn new(semantic_threshold: f32, quality_threshold: f32) -> Self {
        Self {
            buffer: [CandidateDescriptor {
                index: 0,
                vector_sim: 0.0,
                quality_metric: 0.0,
                bm25_score: 0.0,
                freshness_score: 0.0,
                graph_score: 0.0,
                example_count: 0,
                final_rank_score: 0.0,
                passed_gates: false,
            }; MAX_STACK_CANDIDATES],
            count: 0,
            semantic_threshold,
            quality_threshold,
        }
    }

    /// Evaluates candidate through Stage 1 dual-gate thresholds and pushes if buffer allows.
    #[inline(always)]
    pub fn ingest_candidate(&mut self, mut desc: CandidateDescriptor) -> bool {
        // Dual-gate filtering
        let passes_semantic = desc.vector_sim >= self.semantic_threshold;
        let passes_quality = desc.quality_metric >= self.quality_threshold;
        desc.passed_gates = passes_semantic && passes_quality;

        if self.count < MAX_STACK_CANDIDATES {
            let example_bonus = if desc.example_count > 0 { 0.05 } else { 0.0 };
            desc.final_rank_score = 0.40 * desc.vector_sim
                + 0.20 * desc.quality_metric
                + 0.15 * desc.bm25_score
                + 0.10 * desc.freshness_score
                + 0.10 * desc.graph_score
                + example_bonus;

            self.buffer[self.count] = desc;
            self.count += 1;
            desc.passed_gates
        } else {
            false
        }
    }

    /// In-place slice sort on stack buffer (zero dynamic heap allocation).
    #[inline]
    pub fn rank_and_slice(&mut self) -> &[CandidateDescriptor] {
        let active = &mut self.buffer[..self.count];
        // Sort descending: items that passed both gates first, then by final score.
        active.sort_unstable_by(|a, b| {
            b.passed_gates
                .cmp(&a.passed_gates)
                .then_with(|| {
                    b.final_rank_score
                        .partial_cmp(&a.final_rank_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        active
    }
}

