// Mode Mentor — typed shape of a guided learning parcours.
//
// A parcours is a discussion whose `mentor_state` column (migration 074) holds
// this structure serialized as JSON. It describes the subject, the six gated
// blocks (comprehension → resources → target → plan → code → bilan), the
// validation lifecycle, and the hint-ladder level consumed. See
// docs/design/mentor-mode.md.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Lifecycle of a parcours.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MentorStatus {
    /// Background AI generation in flight — the parcours exists but is empty
    /// until the generator workflow fills it (see `generation_error` on failure).
    Generating,
    /// Legacy: a parcours that hasn't opened yet. New parcours open straight to
    /// the learner; kept for back-compat deserialization of older rows.
    Draft,
    /// Legacy validated state — kept for back-compat deserialization.
    Validated,
    /// Opened to the learner (in progress).
    Open,
    /// Completed.
    Done,
}

/// The six blocks/phases of a parcours, in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MentorPhase {
    Comprehension,
    Resources,
    Target,
    Plan,
    Code,
    Bilan,
}

/// Lifecycle of a "Coup de pouce" (graded hint) generated server-side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HintStatus {
    /// Generation in flight — the background task is running the mentor-hint
    /// workflow. Survives a page reload; the UI polls until it settles.
    Pending,
    /// Vetted nudge ready (`text` populated).
    Ready,
    /// The censeur blocked the nudge (would leak the solution) — fail-closed.
    Filtered,
    /// Generation failed (`error` populated).
    Failed,
}

/// The most recent "Coup de pouce" requested on a block. Persisted on the
/// parcours so the nudge survives navigating away and back, and so the run
/// keeps going server-side even if the learner closes the page.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HintState {
    /// The learner block this hint was requested on (a hint from another block
    /// is not shown on the current one).
    pub block: MentorPhase,
    /// The hint-ladder rung this nudge corresponds to (1..=HINT_MAX).
    pub level: u32,
    /// Generation lifecycle.
    pub status: HintStatus,
    /// The vetted nudge text — set only when `status == Ready`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Human-readable error — set only when `status == Failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Outcome of a completed hint generation, folded back into the parcours by
/// [`MentorState::finish_hint`]. Internal — not exposed to the front.
pub enum HintOutcome {
    /// Censeur cleared the nudge — reveal `text`.
    Ready(String),
    /// Censeur blocked the nudge (fail-closed).
    Filtered,
    /// Generation errored.
    Failed(String),
}

/// The mentor's "closure synthesis" for a completed parcours — a recap of what
/// was learned, generated server-side once the learner finishes. Persisted so it
/// survives navigating away and keeps running in the background. Reuses
/// [`HintStatus`] for its lifecycle (`filtered` is unused — no censeur here).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BilanSynthesis {
    pub status: HintStatus,
    /// The recap (Markdown) — set only when `status == Ready`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Human-readable error — set only when `status == Failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The most recent live mentor→censeur→evaluateur turn on a learner block. The
/// turn runs SERVER-SIDE (see `api::mentor::run_turn`) so the mentor's RAW answer
/// never reaches the browser — the client only ever sees the censeur-vetted reply
/// folded into `block.turns`. Persisted (like [`HintState`]) so the turn survives
/// navigating away and the UI can poll until it settles.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TurnState {
    /// The learner block this turn ran on.
    pub block: MentorPhase,
    /// Reuses the hint lifecycle: `Pending` → `Ready` (reply kept) / `Filtered`
    /// (censeur blocked it, fail-closed) / `Failed`.
    pub status: HintStatus,
    /// Human-readable error — set only when `status == Failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Outcome of a completed server-side turn, folded in by [`MentorState::finish_turn`].
/// Internal — never serialized to the front (the front reads the vetted `block.turns`).
pub enum TurnOutcome {
    /// Censeur-vetted: `reply` is `Some` when cleared (`leak == false`), `None`
    /// when filtered. `ready` is the evaluateur's block-approval verdict.
    Done {
        reply: Option<String>,
        ready: Option<bool>,
    },
    /// The run errored.
    Failed(String),
}

/// Where the parcours comes from: a tracker ticket, or a free-form subject.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MentorSource {
    /// "jira" | "free".
    #[serde(rename = "type")]
    pub kind: String,
    /// Ticket key when `kind == "jira"` (e.g. "EW-2481"); `None` for free subjects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_key: Option<String>,
}

/// A resource curated by the mentor (block ② Resources) — a pointer to read,
/// never the how-to itself.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MentorResource {
    pub title: String,
    pub url: String,
    /// "doc" | "article" | "repo" | ...
    pub kind: String,
    #[serde(default)]
    pub read: bool,
}

/// Generic state of a learner-driven block (comprehension / plan / code /
/// bilan): whether it is unlocked (gating), validated by the mentor, the
/// learner's submitted content, and how many revisions were submitted.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MentorBlock {
    pub unlocked: bool,
    #[serde(default)]
    pub validated: bool,
    /// Set by the turn's `evaluateur` verdict: the mentor deems the learner's
    /// submission good enough to move on. Precondition for `advance` on a learner
    /// block (hard gate) — reset on each turn, overridable via the self-serve
    /// "Passer outre" escape hatch.
    #[serde(default)]
    pub mentor_approved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learner: Option<String>,
    #[serde(default)]
    pub revisions: u32,
    /// Persisted dialogue for this block: each learner submission and the
    /// mentor's vetted reply. Kept so the exchange survives validation and page
    /// reloads (the live turn is otherwise ephemeral). Newest last, bounded.
    #[serde(default)]
    pub turns: Vec<MentorTurn>,
    /// Set when the learner used the self-serve "Passer outre" override to move
    /// past the mentor-approval gate. Surfaced so a manually-unblocked pass is
    /// never mistaken for the mentor's own sign-off. Sticky once set.
    #[serde(default)]
    pub forced: bool,
}

/// One recorded exchange on a learner block: what the learner submitted and the
/// mentor's vetted reply (`None` when the censeur filtered it out).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MentorTurn {
    pub submission: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
}

/// Which posture a parcours runs in. `Mentor` = socratic, strict, censor-gated
/// (default → back-compat for existing rows). `Onboarding` = expository course
/// (chapters + checkpoints), no censor. See docs/design/mentor-mode.md.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MentorMode {
    #[default]
    Mentor,
    Onboarding,
}

/// A comprehension checkpoint at the end of an onboarding chapter. A quiz has
/// non-empty `options` + an `answer` index; a free "try it yourself" exercise
/// leaves `options` empty and uses `reveal` for the expected takeaway.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Checkpoint {
    pub question: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<u32>,
    /// Per-option feedback, parallel to `options` (same index/length): why each
    /// choice is right or wrong. Shown after the learner picks, so a wrong answer
    /// teaches (addresses the misconception) instead of just being marked wrong.
    /// Empty (legacy row / none authored) → generic feedback.
    #[serde(default)]
    pub explanations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reveal: Option<String>,
}

/// One chapter of an onboarding course: an explanation (the "why" + real code),
/// an optional checkpoint, and whether the learner has completed it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Chapter {
    pub title: String,
    pub explanation: String,
    /// Legacy SINGLE checkpoint (pre-multi-question). Kept only so courses
    /// generated before `checkpoints` still deserialize; folded into the list by
    /// [`Chapter::effective_checkpoints`]. New courses populate `checkpoints`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<Checkpoint>,
    /// Checkpoints for this chapter (1..N). A content chapter carries one; the
    /// final "Révision" chapter carries a cumulative quiz of several.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checkpoints: Vec<Checkpoint>,
    #[serde(default)]
    pub done: bool,
    /// The learner's own answer to an open-question checkpoint, kept so a
    /// completed chapter shows what they wrote. `None` for quizzes / no answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learner_answer: Option<String>,
    /// Per-learner flag: set when the learner needed more than one attempt to
    /// pass a quiz in this chapter. Drives the end-of-course spaced re-test — a
    /// review pass replays only the chapters flagged here (retrieval practice on
    /// the weak items). Cleared when the chapter is re-passed cleanly.
    #[serde(default)]
    pub needs_review: bool,
}

impl Chapter {
    /// This chapter's checkpoints, folding the legacy singular `checkpoint` into
    /// the list for back-compat. Read this instead of the raw fields.
    pub fn effective_checkpoints(&self) -> Vec<Checkpoint> {
        if !self.checkpoints.is_empty() {
            self.checkpoints.clone()
        } else if let Some(cp) = &self.checkpoint {
            vec![cp.clone()]
        } else {
            Vec::new()
        }
    }
}

/// Full structured state of a Mode Mentor parcours, stored as JSON in
/// `discussions.mentor_state`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MentorState {
    pub status: MentorStatus,
    pub source: MentorSource,
    /// The objective in one or two sentences.
    pub objective: String,
    /// Success criteria (the "done").
    #[serde(default)]
    pub criteria: Vec<String>,
    /// The block the learner is currently on.
    pub phase: MentorPhase,

    // ── Blocks ──
    pub comprehension: MentorBlock,
    /// Block ② — curated resources.
    #[serde(default)]
    pub resources: Vec<MentorResource>,
    /// Block ③ — target architecture ("where we're going"), strict-absolu safe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_archi: Option<String>,
    /// Block ③ — acceptance tests ("how we know it's done").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_tests: Option<String>,
    pub plan: MentorBlock,
    pub code: MentorBlock,
    pub bilan: MentorBlock,

    /// Hint-ladder level consumed on the current block (0 = none given yet).
    #[serde(default)]
    pub hint_level: u32,
    /// The most recent "Coup de pouce" — generated server-side, vetted by the
    /// censeur, persisted so it survives a reload and keeps running if the
    /// learner navigates away. Reset when the block advances.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_hint: Option<HintState>,
    /// The most recent live mentor→censeur→evaluateur turn — run server-side and
    /// censeur-vetted before anything reaches the browser. `Pending` while the run
    /// is in flight; the UI polls until it settles. Reset when the block advances.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_turn: Option<TurnState>,
    /// The mentor's closure synthesis — generated when the parcours completes
    /// (learner-first in mentor mode, direct recap in onboarding). `None` until
    /// completion; persisted so it survives a reload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bilan_synthesis: Option<BilanSynthesis>,

    // ── Posture ──
    /// Pedagogical posture. Defaults to `Mentor` (back-compat for existing rows).
    #[serde(default)]
    pub mode: MentorMode,
    /// Course chapters — used when `mode == Onboarding`. Empty in mentor mode.
    #[serde(default)]
    pub chapters: Vec<Chapter>,

    /// Set when a background generation run failed — the parcours stays in
    /// `Generating` status and the UI shows the error (with a delete affordance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_error: Option<String>,

    /// Source onboarding topic id (from the registry) this parcours was generated
    /// from — `None` for a free mentor parcours. Lets the catalogue detect an
    /// existing parcours for a topic (resume instead of duplicate). See
    /// `OnboardingTopic::topic_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<String>,
    /// Registry topic level ("débutant" / "intermédiaire" / "avancé" — free
    /// string) this parcours was generated from. Carried so the landing list can
    /// badge it without re-reading the registry. `None` for a free parcours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// Registry topic curriculum kind ("tronc" / "branche" / "capstone" /
    /// "culture") — same purpose as `level`. `None` for a free parcours.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// Highest hint-ladder rung (0..=HINT_MAX). Rung 4 = the non-blocking escape
/// (pause / move on and come back / ask a human if there is one), never a code
/// fragment — see the strict directive + mentor-hint.json.
pub const HINT_MAX: u32 = 4;

impl MentorState {
    /// Blocks in their gated order. Resources/Target are informational blocks
    /// the mentor fills (always viewable once reached); the other four are
    /// learner-driven `MentorBlock`s.
    pub const PHASE_ORDER: [MentorPhase; 6] = [
        MentorPhase::Comprehension,
        MentorPhase::Resources,
        MentorPhase::Target,
        MentorPhase::Plan,
        MentorPhase::Code,
        MentorPhase::Bilan,
    ];

    /// The learner-driven block for a phase, if any (Resources/Target have none).
    fn learner_block_mut(&mut self, phase: &MentorPhase) -> Option<&mut MentorBlock> {
        match phase {
            MentorPhase::Comprehension => Some(&mut self.comprehension),
            MentorPhase::Plan => Some(&mut self.plan),
            MentorPhase::Code => Some(&mut self.code),
            MentorPhase::Bilan => Some(&mut self.bilan),
            MentorPhase::Resources | MentorPhase::Target => None,
        }
    }

    /// Read-only twin of [`Self::learner_block_mut`].
    fn learner_block(&self, phase: &MentorPhase) -> Option<&MentorBlock> {
        match phase {
            MentorPhase::Comprehension => Some(&self.comprehension),
            MentorPhase::Plan => Some(&self.plan),
            MentorPhase::Code => Some(&self.code),
            MentorPhase::Bilan => Some(&self.bilan),
            MentorPhase::Resources | MentorPhase::Target => None,
        }
    }

    /// Read-only precondition check for a hint on `block`: validates it's an
    /// unlocked learner block and returns the rung the next hint WOULD reach,
    /// WITHOUT mutating. Lets the caller validate + persist the run row before
    /// committing the `Pending` state, so a later failure can't strand a hint in
    /// `Pending` forever (mirrors the bilan "validate-before-mutate" rule).
    pub fn preview_hint(&self, block: &MentorPhase) -> Result<u32, String> {
        let b = self
            .learner_block(block)
            .ok_or_else(|| "this block does not accept hints".to_string())?;
        if !b.unlocked {
            return Err("block is locked".to_string());
        }
        Ok((self.hint_level + 1).min(HINT_MAX))
    }

    /// Open the parcours to the learner and unlock the first block. Used by the
    /// state-machine tests; new parcours open straight to this state on create.
    pub fn open_to_learner(&mut self) {
        self.status = MentorStatus::Open;
        self.comprehension.unlocked = true;
    }

    /// Cap a learner submission so `mentor_state` can't grow without bound (a code
    /// diff can be large — the Code block assembles the full diff). UTF-8-safe
    /// (`chars().take`, never a byte slice); marks the cut.
    fn truncate_submission(s: String) -> String {
        const MAX_SUBMISSION: usize = 20_000;
        if s.chars().count() > MAX_SUBMISSION {
            let mut t: String = s.chars().take(MAX_SUBMISSION).collect();
            t.push_str("\n\n[…tronqué…]");
            t
        } else {
            s
        }
    }

    /// Store the learner's submission for a block. Only the four learner blocks
    /// accept a submission, and only once unlocked. Capped (see truncate_submission).
    pub fn submit(&mut self, phase: &MentorPhase, content: String) -> Result<(), String> {
        let block = self
            .learner_block_mut(phase)
            .ok_or_else(|| "this block does not accept a learner submission".to_string())?;
        if !block.unlocked {
            return Err("block is locked".to_string());
        }
        block.learner = Some(Self::truncate_submission(content));
        block.revisions += 1;
        Ok(())
    }

    /// Append a completed exchange (learner submission + mentor reply) to a
    /// block's persisted dialogue. `reply == None` records a censeur-filtered
    /// turn. Submissions are capped (a code diff can be large) and history is
    /// bounded so `mentor_state` can't grow without limit.
    pub fn record_turn(
        &mut self,
        phase: &MentorPhase,
        submission: String,
        reply: Option<String>,
    ) -> Result<(), String> {
        const MAX_TURNS: usize = 20;
        let block = self
            .learner_block_mut(phase)
            .ok_or_else(|| "this block has no dialogue".to_string())?;
        let submission = Self::truncate_submission(submission);
        block.turns.push(MentorTurn { submission, reply });
        let overflow = block.turns.len().saturating_sub(MAX_TURNS);
        if overflow > 0 {
            block.turns.drain(0..overflow);
        }
        Ok(())
    }

    /// A compact transcript of the last `max` recorded exchanges on `block`,
    /// oldest first — fed to the mentor turn as context so it builds on the
    /// dialogue instead of repeating itself. Each part is truncated (a Code diff
    /// can be huge, UTF-8-safe via `chars().take`). Empty when there's no prior
    /// turn (first exchange) or the phase carries no dialogue.
    pub fn recent_dialogue(&self, block: &MentorPhase, max: usize) -> String {
        const PART_MAX: usize = 600;
        let Some(b) = self.learner_block(block) else {
            return String::new();
        };
        if b.turns.is_empty() {
            return String::new();
        }
        let start = b.turns.len().saturating_sub(max);
        let mut out = String::new();
        for (i, turn) in b.turns[start..].iter().enumerate() {
            let learner: String = turn.submission.chars().take(PART_MAX).collect();
            let mentor: String = match &turn.reply {
                Some(r) => r.chars().take(PART_MAX).collect(),
                None => "(réponse filtrée par le garde-fou)".to_string(),
            };
            out.push_str(&format!(
                "Échange {} :\n- Apprenti : {}\n- Toi (mentor) : {}\n\n",
                start + i + 1,
                learner.trim(),
                mentor.trim()
            ));
        }
        out.trim_end().to_string()
    }

    /// Mark a phase's block validated and advance the gate: move `phase` to the
    /// next block and unlock its learner block. Resets the hint ladder. The last
    /// phase completes the parcours.
    ///
    /// Hard gates (unless `force`, the self-serve "Passer outre" override):
    /// - Resources: every curated resource must be marked read.
    /// - Learner blocks (comprehension/plan/code/bilan): the mentor must have
    ///   approved the last submission (`mentor_approved`) — no self-validating an
    ///   unreviewed or rejected block.
    pub fn advance(&mut self, phase: &MentorPhase, force: bool) -> Result<(), String> {
        // Ordering gate — applies even to the "Passer outre" override: you can only
        // act on the block you're currently on, never reach forward and skip phases.
        // (force only bypasses the read/approval gate, not the block order.)
        if *phase != self.phase {
            return Err("not the current block".to_string());
        }
        if !force {
            // Resources read-gate (mirrors the front's `resources.every(r => r.read)`).
            if *phase == MentorPhase::Resources && !self.resources.iter().all(|r| r.read) {
                return Err("read every resource before advancing".to_string());
            }
            // Learner-block approval gate: the mentor must have signed off.
            if self
                .learner_block_mut(phase)
                .is_some_and(|b| !b.mentor_approved)
            {
                return Err("the mentor has not approved this block yet".to_string());
            }
        }
        if let Some(block) = self.learner_block_mut(phase) {
            block.validated = true;
            // Record a manual override so the learner sees the block was passed
            // through rather than approved by the mentor.
            if force {
                block.forced = true;
            }
        }
        let idx = Self::PHASE_ORDER
            .iter()
            .position(|p| p == phase)
            .ok_or_else(|| "unknown phase".to_string())?;
        match Self::PHASE_ORDER.get(idx + 1) {
            Some(next) => {
                self.phase = next.clone();
                if let Some(nb) = self.learner_block_mut(next) {
                    nb.unlocked = true;
                }
            }
            None => self.status = MentorStatus::Done,
        }
        self.hint_level = 0;
        self.last_hint = None;
        self.last_turn = None;
        Ok(())
    }

    /// Consume one rung of the hint ladder, capped at `HINT_MAX`. Returns the
    /// new level.
    pub fn hint(&mut self) -> u32 {
        self.hint_level = (self.hint_level + 1).min(HINT_MAX);
        self.hint_level
    }

    /// Read-only precondition for a live turn on `block`: it must be an unlocked
    /// learner block. Mirrors [`Self::preview_hint`] — lets the caller validate +
    /// insert the run row BEFORE committing the `Pending` turn, so a failure can't
    /// strand a turn in `Pending`.
    pub fn preview_turn(&self, block: &MentorPhase) -> Result<(), String> {
        let b = self
            .learner_block(block)
            .ok_or_else(|| "this block does not accept a turn".to_string())?;
        if !b.unlocked {
            return Err("block is locked".to_string());
        }
        Ok(())
    }

    /// Begin a live turn on `block`: store the learner's submission and mark a
    /// `Pending` turn. Errors if `block` isn't an unlocked learner block. The
    /// mentor answer is generated and censeur-vetted SERVER-SIDE (see
    /// `api::mentor::run_turn`), so the raw answer never reaches the client.
    pub fn begin_turn(&mut self, block: MentorPhase, submission: String) -> Result<(), String> {
        self.submit(&block, submission)?; // validates unlocked learner block + stores it
        self.last_turn = Some(TurnState {
            block,
            status: HintStatus::Pending,
            error: None,
        });
        Ok(())
    }

    /// Fold a completed server-side turn into the parcours. No-op if superseded
    /// (block changed, or not `Pending`) — matched like [`Self::finish_hint`]. On
    /// success it appends the censeur-vetted reply to the block dialogue and records
    /// the evaluateur's approval verdict: the whole anti-solution gate lives here,
    /// server-side, so a crafted client request can't reveal a filtered answer.
    pub fn finish_turn(&mut self, block: &MentorPhase, submission: String, outcome: TurnOutcome) {
        match &self.last_turn {
            Some(t) if t.block == *block && t.status == HintStatus::Pending => {}
            _ => return, // superseded — drop the stale completion
        }
        match outcome {
            TurnOutcome::Done { reply, ready } => {
                let filtered = reply.is_none();
                let _ = self.record_turn(block, submission, reply);
                if let Some(approved) = ready {
                    let _ = self.set_block_approval(block, approved);
                }
                if let Some(t) = &mut self.last_turn {
                    t.status = if filtered {
                        HintStatus::Filtered
                    } else {
                        HintStatus::Ready
                    };
                }
            }
            TurnOutcome::Failed(e) => {
                if let Some(t) = &mut self.last_turn {
                    t.status = HintStatus::Failed;
                    t.error = Some(e);
                }
            }
        }
    }

    /// Begin a "Coup de pouce" on `block`: bump the ladder and mark a Pending
    /// hint. Errors if `block` isn't an unlocked learner block. Returns the new
    /// ladder level (so the caller can seed the generation with it).
    pub fn begin_hint(&mut self, block: MentorPhase) -> Result<u32, String> {
        let b = self
            .learner_block_mut(&block)
            .ok_or_else(|| "this block does not accept hints".to_string())?;
        if !b.unlocked {
            return Err("block is locked".to_string());
        }
        let level = self.hint();
        self.last_hint = Some(HintState {
            block,
            level,
            status: HintStatus::Pending,
            text: None,
            error: None,
        });
        Ok(level)
    }

    /// Fold a completed hint generation into the parcours. No-op if the pending
    /// hint was superseded meanwhile (block advanced, or a newer hint started) —
    /// matched on `(block, level)` and only while still `Pending`.
    pub fn finish_hint(&mut self, block: &MentorPhase, level: u32, outcome: HintOutcome) {
        match &mut self.last_hint {
            Some(h) if h.block == *block && h.level == level && h.status == HintStatus::Pending => {
                match outcome {
                    HintOutcome::Ready(text) => {
                        h.status = HintStatus::Ready;
                        h.text = Some(text);
                    }
                    // Filtered/failed = the learner got no usable nudge, so refund
                    // the rung (this hint didn't really consume one). But ONLY when
                    // a rung was actually consumed: at `HINT_MAX`, `hint()` caps and
                    // bumps nothing, so refunding would wrongly step BACK off the
                    // terminal escape rung. Guard proves `hint_level == level`.
                    HintOutcome::Filtered => {
                        h.status = HintStatus::Filtered;
                        if level < HINT_MAX {
                            self.hint_level = self.hint_level.saturating_sub(1);
                        }
                    }
                    HintOutcome::Failed(e) => {
                        h.status = HintStatus::Failed;
                        h.error = Some(e);
                        if level < HINT_MAX {
                            self.hint_level = self.hint_level.saturating_sub(1);
                        }
                    }
                }
            }
            _ => { /* superseded — drop the stale completion */ }
        }
    }

    /// Mark the closure synthesis as generating. Returns `false` (a no-op) unless
    /// the parcours is `Done` and no synthesis is already Pending/Ready — so an
    /// auto-trigger on completion and a manual retry can both call it safely.
    /// A `Failed` synthesis may be retried.
    pub fn begin_bilan(&mut self) -> bool {
        if self.status != MentorStatus::Done {
            return false;
        }
        if matches!(
            self.bilan_synthesis.as_ref().map(|b| b.status),
            Some(HintStatus::Pending) | Some(HintStatus::Ready)
        ) {
            return false;
        }
        self.bilan_synthesis = Some(BilanSynthesis {
            status: HintStatus::Pending,
            text: None,
            error: None,
        });
        true
    }

    /// Fold a completed synthesis run into the parcours. No-op unless a synthesis
    /// is still `Pending` (guards against a superseded/duplicate completion).
    pub fn finish_bilan(&mut self, outcome: HintOutcome) {
        match &mut self.bilan_synthesis {
            Some(b) if b.status == HintStatus::Pending => match outcome {
                HintOutcome::Ready(text) => {
                    b.status = HintStatus::Ready;
                    b.text = Some(text);
                }
                HintOutcome::Filtered => {
                    b.status = HintStatus::Failed;
                    b.error = Some("filtré".into());
                }
                HintOutcome::Failed(e) => {
                    b.status = HintStatus::Failed;
                    b.error = Some(e);
                }
            },
            _ => { /* superseded */ }
        }
    }

    /// A fresh parcours: everything locked, phase at the first block.
    /// Resources/target are filled later (by the setup flow). Callers open it to
    /// the learner (status `Open`) right after seeding.
    pub fn new_draft(source: MentorSource, objective: String, criteria: Vec<String>) -> Self {
        Self {
            status: MentorStatus::Draft,
            source,
            objective,
            criteria,
            phase: MentorPhase::Comprehension,
            comprehension: MentorBlock::default(),
            resources: vec![],
            target_archi: None,
            target_tests: None,
            plan: MentorBlock::default(),
            code: MentorBlock::default(),
            bilan: MentorBlock::default(),
            hint_level: 0,
            last_hint: None,
            last_turn: None,
            bilan_synthesis: None,
            mode: MentorMode::Mentor,
            chapters: vec![],
            generation_error: None,
            topic_id: None,
            level: None,
            kind: None,
        }
    }

    /// A fresh onboarding course (expository posture): chapters to walk through,
    /// no socratic gating and no censor. The mentor-block fields stay at their
    /// defaults (unused in this mode).
    pub fn new_onboarding(source: MentorSource, objective: String, chapters: Vec<Chapter>) -> Self {
        Self {
            mode: MentorMode::Onboarding,
            chapters,
            // No draft gate in onboarding — it's usable right away, so
            // it opens directly instead of sitting in `Draft` forever.
            status: MentorStatus::Open,
            ..Self::new_draft(source, objective, vec![])
        }
    }

    /// Mark curated resource #index read/unread (block ② Resources). Persisted so
    /// the resources read-gate and `progress` reflect the learner's real reading
    /// state (drives whether `advance(Resources)` is allowed).
    pub fn set_resource_read(&mut self, index: usize, read: bool) -> Result<(), String> {
        let res = self
            .resources
            .get_mut(index)
            .ok_or_else(|| "resource index out of range".to_string())?;
        res.read = read;
        Ok(())
    }

    /// Record the mentor's approval verdict for a learner block (the turn's
    /// `evaluateur` result). Only the four learner blocks carry approval;
    /// Resources/Target don't. Errors on a non-learner phase.
    pub fn set_block_approval(
        &mut self,
        phase: &MentorPhase,
        approved: bool,
    ) -> Result<(), String> {
        let block = self
            .learner_block_mut(phase)
            .ok_or_else(|| "this block has no mentor approval".to_string())?;
        block.mentor_approved = approved;
        Ok(())
    }

    /// Mark an onboarding chapter completed. Errors on an out-of-range index (so
    /// the endpoint surfaces a real error instead of a silent `success: true` that
    /// makes the client believe a non-existent chapter was validated).
    /// `answer` persists the learner's open-question response when present.
    /// `needs_review` records that the learner struggled (needed >1 attempt on a
    /// quiz) — the end-of-course review re-tests only flagged chapters. Passing
    /// `false` on a clean (re-)pass clears the flag.
    pub fn complete_chapter(
        &mut self,
        index: usize,
        answer: Option<String>,
        needs_review: bool,
    ) -> Result<(), String> {
        let chapter = self
            .chapters
            .get_mut(index)
            .ok_or_else(|| "chapter index out of range".to_string())?;
        chapter.done = true;
        chapter.needs_review = needs_review;
        if let Some(a) = answer.filter(|a| !a.trim().is_empty()) {
            chapter.learner_answer = Some(a);
        }
        // Onboarding has no draft/validate gate — keep its status in sync with
        // chapter progress (also lifts any legacy row stuck in `Draft`).
        if self.mode == MentorMode::Onboarding {
            self.status = if !self.chapters.is_empty() && self.chapters.iter().all(|c| c.done) {
                MentorStatus::Done
            } else {
                MentorStatus::Open
            };
        }
        Ok(())
    }

    /// Progress as `(done, total)`. Onboarding counts completed chapters;
    /// mentor counts validated blocks over the six gated blocks (mirrors the
    /// front `gatedBlocks` logic so the landing list and the parcours view
    /// agree). Used by the parcours list endpoint.
    pub fn progress(&self) -> (u32, u32) {
        match self.mode {
            MentorMode::Onboarding => (
                self.chapters.iter().filter(|c| c.done).count() as u32,
                self.chapters.len() as u32,
            ),
            MentorMode::Mentor => {
                // Resources and Target have no `validated` flag of their own. Each
                // counts as done once the learner has actually advanced PAST it
                // (clicked "validate this block") — not merely because every resource
                // is ticked, nor because the generator pre-filled archi + tests. This
                // mirrors the front `gatedBlocks` rule so the landing count agrees,
                // and stays correct after a "Passer outre" (which leaves `read` false).
                let phase_idx = Self::PHASE_ORDER.iter().position(|p| *p == self.phase);
                let is_done = self.status == MentorStatus::Done;
                let resources_done = is_done
                    || phase_idx
                        > Self::PHASE_ORDER
                            .iter()
                            .position(|p| *p == MentorPhase::Resources);
                let target_done = is_done
                    || phase_idx
                        > Self::PHASE_ORDER
                            .iter()
                            .position(|p| *p == MentorPhase::Target);
                let done = [
                    self.comprehension.validated,
                    resources_done,
                    target_done,
                    self.plan.validated,
                    self.code.validated,
                    self.bilan.validated,
                ]
                .iter()
                .filter(|b| **b)
                .count() as u32;
                (done, 6)
            }
        }
    }
}

/// One row of the parcours landing list (`GET /api/mentor/parcours`): enough to
/// render a card and open it, without shipping the whole `MentorState`.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ParcoursSummary {
    pub disc_id: String,
    pub title: String,
    pub mode: MentorMode,
    pub status: MentorStatus,
    pub objective: String,
    pub source: MentorSource,
    pub progress_done: u32,
    pub progress_total: u32,
    /// ISO-8601 last-updated timestamp (for ordering / "reprendre" hints).
    pub updated_at: String,
    /// Set when a background generation failed (status stays `generating`) — lets
    /// the card show a failed state instead of a spinner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_error: Option<String>,
    /// Linked project (onboarding is always project-scoped; mentor is usually
    /// null) — lets the landing list group parcours by project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    /// Source onboarding topic id (if generated from a registry topic) — lets the
    /// catalogue match a topic to its existing parcours (resume vs duplicate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<String>,
    /// Registry level / curriculum kind (from the source topic) — lets the
    /// landing card badge a parcours' difficulty and role at a glance. `None` for
    /// a free parcours (no source topic).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

/// `POST /api/mentor/parcours/{id}/submit` body.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SubmitBlockRequest {
    pub block: MentorPhase,
    pub content: String,
}

/// `POST /api/mentor/parcours/{id}/turn` body — run a live mentor→censeur→
/// evaluateur turn on `block` for the learner's `submission`. The mentor answer
/// is generated + censeur-vetted SERVER-SIDE; the client sends only the raw
/// submission and never receives (nor supplies) an unvetted reply. Returns the
/// parcours with `last_turn.status == "pending"`; poll `getParcours` to settle.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct RunTurnRequest {
    pub block: MentorPhase,
    pub submission: String,
}

/// `POST /api/mentor/parcours/{id}/advance` body. `force` is the self-serve
/// "Passer outre" override — bypass the read/approval gates (default false).
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct AdvanceBlockRequest {
    pub block: MentorPhase,
    #[serde(default)]
    pub force: bool,
}

/// `POST /api/mentor/parcours/{id}/hint` body — request a graded "Coup de
/// pouce" on `block`. `submission` is what the learner has typed so far (may be
/// empty). The `subject` and project anchor are derived server-side.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct RequestHintRequest {
    pub block: MentorPhase,
    #[serde(default)]
    pub submission: String,
}

/// `POST /api/mentor/parcours/{id}/chapter` body — mark an onboarding chapter
/// completed (unlocks the next one). Index is 0-based into `chapters`.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CompleteChapterRequest {
    pub index: u32,
    /// Learner's answer to an open-question checkpoint (persisted for review).
    #[serde(default)]
    pub answer: Option<String>,
    /// True when the learner needed more than one attempt on a quiz in this
    /// chapter — flags it for the end-of-course spaced re-test (#4b).
    #[serde(default)]
    pub needs_review: bool,
}

/// `POST /api/mentor/parcours/{id}/resource-read` body — mark curated resource
/// #index read/unread. Index is 0-based into `resources`.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct SetResourceReadRequest {
    pub index: u32,
    pub read: bool,
}

/// A curated onboarding topic from a project's `docs/onboarding.md` registry.
/// The registry is a doc-IA artifact (like `inconsistencies-tech-debt.md`):
/// human-curated AND proposed by the onboarding audit agent (O4b). The
/// onboarding posture reads these as a catalogue → generates a chapter course
/// anchored on the referenced files. Parsed by `core::onboarding_registry`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OnboardingTopic {
    pub title: String,
    /// Stable identifier for the topic — an explicit `**ID**` bullet if the
    /// registry pins one, else a deterministic slug of the title. Lets a
    /// generated parcours be matched back to its source topic (dedup / resume)
    /// independently of the display title. See `core::onboarding_registry`.
    #[serde(default)]
    pub topic_id: String,
    /// Curriculum role, normalized to "tronc" | "branche" | "capstone" | "culture"
    /// (else the raw lowercased value). `None` when the topic carries no `Type`
    /// bullet. Lets the catalogue group a flat registry into a trunk → branches →
    /// capstone → culture curriculum. Back-compat: absent on legacy rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// "débutant" | "intermédiaire" | "avancé" | free text; None if unspecified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// One-line scope ("périmètre").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prerequisites: Option<String>,
    /// Reference files/docs to anchor the generated course.
    #[serde(default)]
    pub references: Vec<String>,
    /// Free-form description below the labelled bullets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Repo-relative path to the persisted course (`docs/onboarding/NN-slug.md`),
    /// present once the onboarding posture has generated its chapters. `None`
    /// while the topic is only catalogued (course not generated yet). Written
    /// from a `- **Cours** :` bullet by `core::onboarding_registry`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub course_path: Option<String>,
}

/// `POST /api/mentor/parcours` body — create a new parcours (a discussion + a
/// fresh draft state).
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct CreateParcoursRequest {
    pub title: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub source: MentorSource,
    pub objective: String,
    #[serde(default)]
    pub criteria: Vec<String>,
    /// Optional pre-filled content (e.g. produced by the generator workflow).
    /// Empty/None on a bare manual create.
    #[serde(default)]
    pub resources: Vec<MentorResource>,
    #[serde(default)]
    pub target_archi: Option<String>,
    #[serde(default)]
    pub target_tests: Option<String>,
    /// Posture (defaults to `Mentor`). `Onboarding` builds a chapter-based course.
    #[serde(default)]
    pub mode: MentorMode,
    /// Course chapters, used when `mode == Onboarding` (e.g. from the generator).
    #[serde(default)]
    pub chapters: Vec<Chapter>,
}

/// Response of `POST /api/mentor/parcours`: the created disc id + its state.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct CreateParcoursResponse {
    pub disc_id: String,
    pub state: MentorState,
}

/// `POST /api/mentor/parcours/generate` body — kick off background AI generation.
/// A placeholder parcours (status `generating`) is created and returned at once;
/// a background task runs the generator workflow and fills it in.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
pub struct GenerateParcoursRequest {
    pub title: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub source: MentorSource,
    /// Placeholder objective shown while generating (the subject or title).
    pub objective: String,
    /// Posture: `Mentor` runs the parcours generator, `Onboarding` the course generator.
    #[serde(default)]
    pub mode: MentorMode,
    /// Generator launch variable — free subject (empty for a Jira source).
    #[serde(default)]
    pub subject: String,
    /// Generator launch variable — Jira ticket key (empty for a free subject).
    #[serde(default)]
    pub ticket_key: String,
    /// Source onboarding topic id (from the registry) — persisted on the parcours
    /// so the catalogue can offer "reprendre" instead of a duplicate. `None` for a
    /// free mentor parcours.
    #[serde(default)]
    pub topic_id: Option<String>,
    /// Registry level / curriculum kind of the source topic — persisted on the
    /// parcours so the landing list can badge it. `None` for a free parcours.
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MentorState {
        MentorState {
            status: MentorStatus::Open,
            source: MentorSource {
                kind: "jira".into(),
                ticket_key: Some("EW-2481".into()),
            },
            objective: "Afficher un skeleton pendant le chargement".into(),
            criteria: vec!["Pas de saut de layout".into()],
            phase: MentorPhase::Plan,
            comprehension: MentorBlock {
                unlocked: true,
                validated: true,
                mentor_approved: true,
                learner: Some("reformulation".into()),
                revisions: 0,
                turns: vec![],
                forced: false,
            },
            resources: vec![MentorResource {
                title: "MDN aria-busy".into(),
                url: "https://mdn".into(),
                kind: "doc".into(),
                read: true,
            }],
            target_archi: Some("ArticleList décide Skeleton|Cards".into()),
            target_tests: Some("test: 6 skeletons pendant le fetch".into()),
            plan: MentorBlock {
                unlocked: true,
                validated: false,
                mentor_approved: false,
                learner: None,
                revisions: 2,
                turns: vec![],
                forced: false,
            },
            code: MentorBlock::default(),
            bilan: MentorBlock::default(),
            hint_level: 1,
            last_hint: None,
            last_turn: None,
            bilan_synthesis: None,
            mode: MentorMode::Mentor,
            chapters: vec![],
            generation_error: None,
            topic_id: None,
            level: None,
            kind: None,
        }
    }

    #[test]
    fn round_trips_through_json() {
        let st = sample();
        let json = serde_json::to_string(&st).expect("serialize");
        let back: MentorState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.status, MentorStatus::Open);
        assert_eq!(back.phase, MentorPhase::Plan);
        assert_eq!(back.plan.revisions, 2);
        assert_eq!(back.source.ticket_key.as_deref(), Some("EW-2481"));
        assert_eq!(back.resources.len(), 1);
    }

    #[test]
    fn json_shape_matches_frontend_contract() {
        let json = serde_json::to_value(sample()).expect("to_value");
        // Enums serialize as snake_case strings (what the design doc + TS expect).
        assert_eq!(json["status"], "open");
        assert_eq!(json["phase"], "plan");
        // Source discriminant is the `type` key (not `kind`).
        assert_eq!(json["source"]["type"], "jira");
    }

    #[test]
    fn tolerates_minimal_json_with_defaults() {
        // A freshly-created free-subject parcours: only the required fields set,
        // everything else falls back to serde defaults.
        let minimal = r#"{
            "status": "draft",
            "source": { "type": "free" },
            "objective": "Comprendre les Web Components",
            "phase": "comprehension",
            "comprehension": { "unlocked": true },
            "plan": { "unlocked": false },
            "code": { "unlocked": false },
            "bilan": { "unlocked": false }
        }"#;
        let st: MentorState = serde_json::from_str(minimal).expect("deserialize minimal");
        assert!(st.criteria.is_empty());
        assert!(st.resources.is_empty());
        assert_eq!(st.hint_level, 0);
        assert_eq!(st.source.ticket_key, None);
        assert!(!st.plan.validated);
    }

    fn fresh_draft() -> MentorState {
        MentorState {
            status: MentorStatus::Draft,
            source: MentorSource {
                kind: "free".into(),
                ticket_key: None,
            },
            objective: "Comprendre les skeletons".into(),
            criteria: vec![],
            phase: MentorPhase::Comprehension,
            comprehension: MentorBlock::default(),
            resources: vec![],
            target_archi: None,
            target_tests: None,
            plan: MentorBlock::default(),
            code: MentorBlock::default(),
            bilan: MentorBlock::default(),
            hint_level: 0,
            last_hint: None,
            last_turn: None,
            bilan_synthesis: None,
            mode: MentorMode::Mentor,
            chapters: vec![],
            generation_error: None,
            topic_id: None,
            level: None,
            kind: None,
        }
    }

    #[test]
    fn open_unlocks_first_block() {
        let mut st = fresh_draft();
        assert!(!st.comprehension.unlocked);
        st.open_to_learner();
        assert_eq!(st.status, MentorStatus::Open);
        assert!(st.comprehension.unlocked);
    }

    #[test]
    fn submit_requires_unlocked_block() {
        let mut st = fresh_draft();
        // Plan is locked at the start.
        assert!(st.submit(&MentorPhase::Plan, "mon plan".into()).is_err());
        // Comprehension unlocks when the parcours opens.
        st.open_to_learner();
        assert!(st
            .submit(&MentorPhase::Comprehension, "ma reformulation".into())
            .is_ok());
        assert_eq!(st.comprehension.revisions, 1);
        assert_eq!(
            st.comprehension.learner.as_deref(),
            Some("ma reformulation")
        );
        // Resources/Target are not learner-submittable.
        assert!(st.submit(&MentorPhase::Resources, "x".into()).is_err());
    }

    #[test]
    fn advance_gates_forward_and_resets_hints() {
        let mut st = fresh_draft();
        st.open_to_learner();
        st.begin_hint(MentorPhase::Comprehension).unwrap(); // consume a hint
        assert_eq!(st.hint_level, 1);
        assert!(st.last_hint.is_some());

        st.comprehension.mentor_approved = true; // mentor signed off
        st.advance(&MentorPhase::Comprehension, false).unwrap();
        assert!(st.comprehension.validated);
        assert_eq!(st.phase, MentorPhase::Resources);
        assert_eq!(st.hint_level, 0); // reset on advance
        assert!(st.last_hint.is_none()); // the stale nudge is cleared too
        assert!(!st.plan.unlocked); // plan still locked

        st.advance(&MentorPhase::Resources, false).unwrap(); // no resources → read-gate passes
        st.advance(&MentorPhase::Target, false).unwrap(); // target has no approval gate
        assert_eq!(st.phase, MentorPhase::Plan);
        assert!(st.plan.unlocked); // reaching Plan unlocks its learner block
    }

    #[test]
    fn hint_lifecycle_begin_and_finish() {
        let mut st = fresh_draft();
        st.open_to_learner();

        // A hint on a locked block is refused.
        assert!(st.begin_hint(MentorPhase::Plan).is_err());

        // Begin a hint on the unlocked comprehension block → Pending at level 1.
        let level = st.begin_hint(MentorPhase::Comprehension).unwrap();
        assert_eq!(level, 1);
        let h = st.last_hint.as_ref().unwrap();
        assert_eq!(h.block, MentorPhase::Comprehension);
        assert_eq!(h.status, HintStatus::Pending);
        assert!(h.text.is_none());

        // A completion for a superseded (wrong) level is dropped.
        st.finish_hint(
            &MentorPhase::Comprehension,
            99,
            HintOutcome::Ready("nope".into()),
        );
        assert_eq!(st.last_hint.as_ref().unwrap().status, HintStatus::Pending);

        // The matching completion folds the vetted nudge in.
        st.finish_hint(
            &MentorPhase::Comprehension,
            1,
            HintOutcome::Ready("Et si tu regardais l'état de chargement ?".into()),
        );
        let h = st.last_hint.as_ref().unwrap();
        assert_eq!(h.status, HintStatus::Ready);
        assert_eq!(
            h.text.as_deref(),
            Some("Et si tu regardais l'état de chargement ?")
        );

        // A second completion no longer applies (already settled, not Pending).
        st.finish_hint(&MentorPhase::Comprehension, 1, HintOutcome::Filtered);
        assert_eq!(st.last_hint.as_ref().unwrap().status, HintStatus::Ready);
    }

    #[test]
    fn hint_filtered_outcome_carries_no_text() {
        let mut st = fresh_draft();
        st.open_to_learner();
        st.begin_hint(MentorPhase::Comprehension).unwrap();
        st.finish_hint(&MentorPhase::Comprehension, 1, HintOutcome::Filtered);
        let h = st.last_hint.as_ref().unwrap();
        assert_eq!(h.status, HintStatus::Filtered);
        assert!(h.text.is_none());
    }

    // M5: a filtered/failed hint gave no usable nudge → the rung is refunded so
    // the learner can retry the same rung instead of "burning" it.
    #[test]
    fn hint_filtered_or_failed_refunds_the_rung() {
        for outcome in [HintOutcome::Filtered, HintOutcome::Failed("boom".into())] {
            let mut st = fresh_draft();
            st.open_to_learner();
            let level = st.begin_hint(MentorPhase::Comprehension).unwrap();
            assert_eq!(level, 1);
            assert_eq!(st.hint_level, 1);
            st.finish_hint(&MentorPhase::Comprehension, level, outcome);
            assert_eq!(st.hint_level, 0, "a fruitless hint must not consume a rung");
            // The next hint therefore reaches rung 1 again, not 2.
            assert_eq!(st.begin_hint(MentorPhase::Comprehension).unwrap(), 1);
        }
        // A successful hint keeps the rung consumed.
        let mut st = fresh_draft();
        st.open_to_learner();
        st.begin_hint(MentorPhase::Comprehension).unwrap();
        st.finish_hint(
            &MentorPhase::Comprehension,
            1,
            HintOutcome::Ready("ok".into()),
        );
        assert_eq!(st.hint_level, 1);
    }

    // B1: preview_hint validates + returns the prospective rung WITHOUT mutating,
    // so the handler can persist the run row before committing the Pending state.
    #[test]
    fn preview_hint_is_read_only_and_validates() {
        let mut st = fresh_draft();
        st.open_to_learner();
        // Locked block → refused, and Resources/Target take no hint.
        assert!(st.preview_hint(&MentorPhase::Plan).is_err());
        assert!(st.preview_hint(&MentorPhase::Resources).is_err());
        // Unlocked block → prospective rung 1, and hint_level is untouched.
        assert_eq!(st.preview_hint(&MentorPhase::Comprehension), Ok(1));
        assert_eq!(st.hint_level, 0);
        // Caps at HINT_MAX.
        st.hint_level = HINT_MAX;
        assert_eq!(st.preview_hint(&MentorPhase::Comprehension), Ok(HINT_MAX));
    }

    // A filtered/failed hint refunds the rung it consumed — BUT not at HINT_MAX,
    // where `hint()` caps and consumes nothing, so a refund there would wrongly
    // step back off the terminal escape rung.
    #[test]
    fn filtered_hint_refunds_a_rung_but_not_at_the_cap() {
        let mut st = fresh_draft();
        st.open_to_learner();
        // Below the cap: rung 1 consumed, then filtered → refunded to 0.
        st.begin_hint(MentorPhase::Comprehension).unwrap();
        assert_eq!(st.hint_level, 1);
        st.finish_hint(&MentorPhase::Comprehension, 1, HintOutcome::Filtered);
        assert_eq!(
            st.hint_level, 0,
            "a consumed rung is refunded when the hint is filtered"
        );

        // At the cap: begin_hint bumps nothing (stays at HINT_MAX), so a filtered
        // outcome must NOT refund — the escape rung stays reached.
        st.hint_level = HINT_MAX;
        st.begin_hint(MentorPhase::Comprehension).unwrap();
        assert_eq!(st.hint_level, HINT_MAX);
        st.finish_hint(&MentorPhase::Comprehension, HINT_MAX, HintOutcome::Filtered);
        assert_eq!(
            st.hint_level, HINT_MAX,
            "no rung was consumed at the cap → no refund"
        );
    }

    // Resources counts as done only once the learner has advanced PAST it — never
    // because the list is vacuously "all read" (empty). A fresh parcours has
    // nothing done. An empty list doesn't strand progress: the read-gate passes on
    // an empty list, so advancing past Resources is always possible.
    #[test]
    fn progress_counts_resources_done_only_after_advancing_past() {
        let mut st = fresh_draft();
        st.open_to_learner();
        assert!(st.resources.is_empty());
        assert_eq!(st.progress(), (0, 6));
        // Advance through comprehension, then past the (empty) resources block.
        st.comprehension.mentor_approved = true;
        st.advance(&MentorPhase::Comprehension, false).unwrap();
        st.advance(&MentorPhase::Resources, false).unwrap();
        // comprehension validated + resources passed → 2/6.
        assert_eq!(st.progress(), (2, 6));
    }

    // M1: an oversized learner submission is capped (a Code-block diff can be huge)
    // so mentor_state can't grow without bound.
    #[test]
    fn submit_caps_oversized_content() {
        let mut st = fresh_draft();
        st.open_to_learner();
        st.submit(&MentorPhase::Comprehension, "x".repeat(25_000))
            .unwrap();
        let stored = st.comprehension.learner.as_ref().unwrap();
        assert!(stored.chars().count() <= 20_020, "capped near 20k + marker");
        assert!(stored.ends_with("[…tronqué…]"));
    }

    #[test]
    fn bilan_synthesis_only_after_completion() {
        let mut st = fresh_draft();
        // Not done yet → begin_bilan is a no-op.
        assert!(!st.begin_bilan());
        assert!(st.bilan_synthesis.is_none());

        // Complete the parcours.
        st.open_to_learner();
        st.bilan.mentor_approved = true;
        // Fast-forward straight to Done for the test.
        st.status = MentorStatus::Done;

        // First begin marks Pending; a second is a no-op (already Pending).
        assert!(st.begin_bilan());
        assert_eq!(
            st.bilan_synthesis.as_ref().unwrap().status,
            HintStatus::Pending
        );
        assert!(!st.begin_bilan());

        // Fold the result in.
        st.finish_bilan(HintOutcome::Ready("## Ce que tu as accompli\n…".into()));
        let b = st.bilan_synthesis.as_ref().unwrap();
        assert_eq!(b.status, HintStatus::Ready);
        assert!(b.text.as_deref().unwrap().starts_with("## Ce que"));

        // A Ready synthesis is not regenerated; a Failed one may be retried.
        assert!(!st.begin_bilan());
        st.bilan_synthesis.as_mut().unwrap().status = HintStatus::Failed;
        assert!(st.begin_bilan());
    }

    #[test]
    fn record_turn_appends_and_bounds_history() {
        let mut st = fresh_draft();
        st.open_to_learner();
        // A turn with a vetted reply, then a filtered one (reply = None).
        st.record_turn(
            &MentorPhase::Comprehension,
            "ma reformulation".into(),
            Some("Et si tu regardais X ?".into()),
        )
        .unwrap();
        st.record_turn(&MentorPhase::Comprehension, "2e essai".into(), None)
            .unwrap();
        assert_eq!(st.comprehension.turns.len(), 2);
        assert_eq!(
            st.comprehension.turns[0].reply.as_deref(),
            Some("Et si tu regardais X ?")
        );
        assert!(st.comprehension.turns[1].reply.is_none());
        // Resources/Target aren't learner blocks → no dialogue.
        assert!(st
            .record_turn(&MentorPhase::Resources, "x".into(), None)
            .is_err());

        // History is bounded (drops the oldest beyond the cap).
        for i in 0..30 {
            st.record_turn(&MentorPhase::Comprehension, format!("t{i}"), None)
                .unwrap();
        }
        assert!(st.comprehension.turns.len() <= 20);
        assert_eq!(st.comprehension.turns.last().unwrap().submission, "t29");
    }

    #[test]
    fn resources_gate_requires_every_resource_read() {
        let mut st = fresh_draft();
        st.open_to_learner();
        st.comprehension.mentor_approved = true;
        st.advance(&MentorPhase::Comprehension, false).unwrap();
        assert_eq!(st.phase, MentorPhase::Resources);

        st.resources = vec![
            MentorResource {
                title: "Doc A".into(),
                url: "https://a".into(),
                kind: "doc".into(),
                read: false,
            },
            MentorResource {
                title: "Repo B".into(),
                url: "https://b".into(),
                kind: "repo".into(),
                read: false,
            },
        ];

        // Not all read → the gate blocks the advance and the phase stays put.
        assert!(st.advance(&MentorPhase::Resources, false).is_err());
        assert_eq!(st.phase, MentorPhase::Resources);

        // Mark them read one by one; only the last one lifts the gate.
        st.set_resource_read(0, true).unwrap();
        assert!(st.advance(&MentorPhase::Resources, false).is_err());
        st.set_resource_read(1, true).unwrap();
        st.advance(&MentorPhase::Resources, false).unwrap();
        assert_eq!(st.phase, MentorPhase::Target);

        // Out-of-range index is a validation error, not a panic.
        assert!(st.set_resource_read(9, true).is_err());
    }

    #[test]
    fn advancing_the_last_block_completes_the_parcours() {
        let mut st = fresh_draft();
        st.open_to_learner();
        // The learner has reached the final block (advance only acts on the
        // current phase now — no reaching forward to skip blocks).
        st.phase = MentorPhase::Bilan;
        st.bilan.unlocked = true;
        st.bilan.mentor_approved = true;
        st.advance(&MentorPhase::Bilan, false).unwrap();
        assert_eq!(st.status, MentorStatus::Done);
    }

    #[test]
    fn advance_rejects_a_block_that_is_not_the_current_phase() {
        let mut st = fresh_draft();
        st.open_to_learner(); // phase = Comprehension
                              // Even with approval + force, you can't reach forward to a later block and
                              // skip the ones in between (the client-side gating-bypass this closes).
        st.code.mentor_approved = true;
        assert!(st.advance(&MentorPhase::Code, false).is_err());
        assert!(st.advance(&MentorPhase::Code, true).is_err());
        assert_eq!(st.phase, MentorPhase::Comprehension);
    }

    #[test]
    fn begin_turn_stores_submission_and_marks_pending() {
        let mut st = fresh_draft();
        st.open_to_learner();
        st.begin_turn(MentorPhase::Comprehension, "ma reformulation".into())
            .unwrap();
        assert_eq!(
            st.comprehension.learner.as_deref(),
            Some("ma reformulation")
        );
        let t = st.last_turn.as_ref().unwrap();
        assert_eq!(t.block, MentorPhase::Comprehension);
        assert_eq!(t.status, HintStatus::Pending);
        // A locked block can't start a turn.
        assert!(st.begin_turn(MentorPhase::Code, "x".into()).is_err());
    }

    #[test]
    fn finish_turn_reveals_only_a_censeur_cleared_reply() {
        let mut st = fresh_draft();
        st.open_to_learner();
        st.begin_turn(MentorPhase::Comprehension, "sub".into())
            .unwrap();
        st.finish_turn(
            &MentorPhase::Comprehension,
            "sub".into(),
            TurnOutcome::Done {
                reply: Some("des questions ouvertes".into()),
                ready: Some(true),
            },
        );
        assert_eq!(st.last_turn.as_ref().unwrap().status, HintStatus::Ready);
        assert!(st.comprehension.mentor_approved);
        assert_eq!(
            st.comprehension.turns.last().unwrap().reply.as_deref(),
            Some("des questions ouvertes")
        );
    }

    #[test]
    fn finish_turn_filters_a_leaking_reply_fail_closed() {
        let mut st = fresh_draft();
        st.open_to_learner();
        st.begin_turn(MentorPhase::Comprehension, "sub".into())
            .unwrap();
        // Censeur blocked the answer (reply None): it must NOT be revealed, and the
        // recorded turn carries no reply. Approval is left untouched (ready None).
        st.finish_turn(
            &MentorPhase::Comprehension,
            "sub".into(),
            TurnOutcome::Done {
                reply: None,
                ready: None,
            },
        );
        assert_eq!(st.last_turn.as_ref().unwrap().status, HintStatus::Filtered);
        assert!(!st.comprehension.mentor_approved);
        assert_eq!(st.comprehension.turns.last().unwrap().reply, None);
    }

    #[test]
    fn recent_dialogue_formats_prior_turns_and_marks_filtered() {
        let mut st = fresh_draft();
        st.open_to_learner();
        // No prior turn → empty (first exchange).
        assert_eq!(st.recent_dialogue(&MentorPhase::Comprehension, 3), "");
        // Two exchanges: one vetted, one filtered.
        st.record_turn(
            &MentorPhase::Comprehension,
            "j'ai pensé à un state".into(),
            Some("qu'observes-tu quand la liste est vide ?".into()),
        )
        .unwrap();
        st.record_turn(
            &MentorPhase::Comprehension,
            "donne-moi le code".into(),
            None,
        )
        .unwrap();
        let d = st.recent_dialogue(&MentorPhase::Comprehension, 3);
        assert!(d.contains("Apprenti : j'ai pensé à un state"));
        assert!(d.contains("Toi (mentor) : qu'observes-tu"));
        assert!(
            d.contains("(réponse filtrée par le garde-fou)"),
            "a filtered turn is marked, not blank"
        );
        // `max` keeps only the most recent.
        let only_last = st.recent_dialogue(&MentorPhase::Comprehension, 1);
        assert!(!only_last.contains("j'ai pensé à un state"));
        assert!(only_last.contains("donne-moi le code"));
        // A phase without dialogue → empty.
        assert_eq!(st.recent_dialogue(&MentorPhase::Resources, 3), "");
    }

    #[test]
    fn finish_turn_is_a_noop_when_superseded() {
        let mut st = fresh_draft();
        st.open_to_learner();
        st.begin_turn(MentorPhase::Comprehension, "sub".into())
            .unwrap();
        // A completion for a DIFFERENT block is a stale/superseded run → dropped.
        st.finish_turn(
            &MentorPhase::Plan,
            "sub".into(),
            TurnOutcome::Done {
                reply: Some("x".into()),
                ready: Some(true),
            },
        );
        assert_eq!(st.last_turn.as_ref().unwrap().status, HintStatus::Pending);
        assert!(st.comprehension.turns.is_empty());
    }

    #[test]
    fn learner_block_advance_requires_mentor_approval() {
        let mut st = fresh_draft();
        st.open_to_learner();
        // Comprehension is unlocked but the mentor hasn't approved → gate blocks.
        assert!(st.advance(&MentorPhase::Comprehension, false).is_err());
        assert_eq!(st.phase, MentorPhase::Comprehension);
        // The self-serve override (force) bypasses the gate.
        st.advance(&MentorPhase::Comprehension, true).unwrap();
        assert_eq!(st.phase, MentorPhase::Resources);
        // With approval recorded, no force needed. Fast-forward to Plan first.
        st.advance(&MentorPhase::Resources, true).unwrap();
        st.advance(&MentorPhase::Target, true).unwrap();
        assert_eq!(st.phase, MentorPhase::Plan);
        assert!(st.advance(&MentorPhase::Plan, false).is_err()); // not approved
        st.set_block_approval(&MentorPhase::Plan, true).unwrap();
        st.advance(&MentorPhase::Plan, false).unwrap();
        assert_eq!(st.phase, MentorPhase::Code);
        // Approval on a non-learner block is rejected.
        assert!(st
            .set_block_approval(&MentorPhase::Resources, true)
            .is_err());
    }

    #[test]
    fn hint_ladder_is_capped() {
        let mut st = fresh_draft();
        for _ in 0..10 {
            st.hint();
        }
        assert_eq!(st.hint_level, HINT_MAX);
    }

    #[test]
    fn onboarding_course_completes_chapters() {
        let mut st = MentorState::new_onboarding(
            MentorSource {
                kind: "free".into(),
                ticket_key: None,
            },
            "Comprendre le rendu des articles".into(),
            vec![
                Chapter {
                    title: "Flux de données".into(),
                    explanation: "…".into(),
                    checkpoint: None,
                    done: false,
                    learner_answer: None,
                    checkpoints: vec![],
                    needs_review: false,
                },
                Chapter {
                    title: "États de chargement".into(),
                    explanation: "…".into(),
                    checkpoint: Some(Checkpoint {
                        question: "Qu'est-ce qui déclenche les skeletons ?".into(),
                        options: vec!["un timer".into(), "isLoading".into()],
                        answer: Some(1),
                        explanations: vec![
                            "Non — un timer arbitraire ne reflète pas l'état réel de la requête."
                                .into(),
                            "Oui — le flag isLoading suit le cycle de vie de la requête.".into(),
                        ],
                        reveal: None,
                    }),
                    checkpoints: vec![],
                    done: false,
                    learner_answer: None,
                    needs_review: false,
                },
            ],
        );
        assert_eq!(st.mode, MentorMode::Onboarding);
        assert_eq!(st.chapters.len(), 2);
        assert!(!st.chapters[0].done);
        // Onboarding opens straight away (no draft gate).
        assert_eq!(st.status, MentorStatus::Open);
        st.complete_chapter(0, None, false).unwrap();
        assert!(st.chapters[0].done);
        assert_eq!(st.status, MentorStatus::Open); // not all done yet
        st.complete_chapter(1, None, false).unwrap();
        assert_eq!(st.status, MentorStatus::Done); // every chapter done → done
        assert!(st.complete_chapter(99, None, false).is_err()); // out of range → error, not a silent success
    }

    #[test]
    fn progress_counts_chapters_and_blocks() {
        // Onboarding: done chapters / total.
        let mut onb = MentorState::new_onboarding(
            MentorSource {
                kind: "free".into(),
                ticket_key: None,
            },
            "x".into(),
            vec![
                Chapter {
                    title: "a".into(),
                    explanation: "".into(),
                    checkpoint: None,
                    done: true,
                    learner_answer: None,
                    checkpoints: vec![],
                    needs_review: false,
                },
                Chapter {
                    title: "b".into(),
                    explanation: "".into(),
                    checkpoint: None,
                    done: false,
                    learner_answer: None,
                    checkpoints: vec![],
                    needs_review: false,
                },
            ],
        );
        assert_eq!(onb.progress(), (1, 2));
        onb.complete_chapter(1, None, false).unwrap();
        assert_eq!(onb.progress(), (2, 2));

        // Mentor: validated blocks over 6. A fresh draft has nothing done → 0/6
        // (Resources/Target only count once advanced past — see the dedicated tests).
        let mut m = fresh_draft();
        assert_eq!(m.progress(), (0, 6));
        m.open_to_learner();
        m.comprehension.mentor_approved = true;
        m.advance(&MentorPhase::Comprehension, false).unwrap(); // comprehension validated
        assert_eq!(m.progress().1, 6);
        assert!(m.progress().0 >= 1);
    }

    #[test]
    fn target_counts_done_only_after_advancing_past_it() {
        let mut st = fresh_draft();
        // The generator pre-fills the target at creation — that alone must NOT
        // make it count as done (the old bug: always "validated").
        st.target_archi = Some("flowchart TD\n  A-->B".into());
        st.target_tests = Some("given/when/then".into());
        st.resources = vec![MentorResource {
            title: "Doc".into(),
            url: "https://d".into(),
            kind: "doc".into(),
            read: true,
        }];

        st.open_to_learner();
        st.comprehension.mentor_approved = true;
        st.advance(&MentorPhase::Comprehension, false).unwrap(); // → resources
        st.advance(&MentorPhase::Resources, false).unwrap(); // → target (resource read)

        // AT target (phase == target) but not past it → target not counted yet.
        let before = st.progress().0;
        st.advance(&MentorPhase::Target, false).unwrap(); // → plan (past target)
        assert_eq!(
            st.progress().0,
            before + 1,
            "target counts only once advanced past"
        );
    }

    #[test]
    fn complete_chapter_request_deserializes() {
        let req: CompleteChapterRequest =
            serde_json::from_str(r#"{ "index": 2 }"#).expect("deserialize");
        assert_eq!(req.index, 2);
    }

    #[test]
    fn onboarding_mode_serializes_snake_case() {
        let st = MentorState::new_onboarding(
            MentorSource {
                kind: "free".into(),
                ticket_key: None,
            },
            "x".into(),
            vec![],
        );
        let json = serde_json::to_value(&st).expect("to_value");
        assert_eq!(json["mode"], "onboarding");
    }

    #[test]
    fn existing_mentor_json_defaults_to_mentor_mode() {
        // Back-compat : a row written before the `mode` field existed.
        let legacy = r#"{
            "status": "open", "source": { "type": "free" }, "objective": "x",
            "phase": "plan", "comprehension": {"unlocked": true}, "plan": {"unlocked": true},
            "code": {"unlocked": false}, "bilan": {"unlocked": false}
        }"#;
        let st: MentorState = serde_json::from_str(legacy).expect("legacy deserialize");
        assert_eq!(st.mode, MentorMode::Mentor);
        assert!(st.chapters.is_empty());
    }

    #[test]
    fn new_draft_starts_locked() {
        let st = MentorState::new_draft(
            MentorSource {
                kind: "free".into(),
                ticket_key: None,
            },
            "Comprendre les Web Components".into(),
            vec!["Sait créer un custom element".into()],
        );
        assert_eq!(st.status, MentorStatus::Draft);
        assert_eq!(st.phase, MentorPhase::Comprehension);
        assert!(!st.comprehension.unlocked); // nothing open until the parcours is opened
        assert!(!st.plan.unlocked);
        assert_eq!(st.hint_level, 0);
        assert_eq!(st.criteria.len(), 1);
    }
}
