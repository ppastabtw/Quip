//! Word-level edit stream (rolling correction, doc 1).
//!
//! Pure logic, no model dependency. [`diff_words`] aligns a model suggestion
//! against the typed draft word by word; [`SessionEdits`] accumulates those
//! edits across overlapping passes into per-word slots and decides when a
//! correction has *hardened* — stable enough to surface as a quiet inline
//! mark instead of an interrupting candidate bar. Hardening is gated on
//! votes (intra-pass confidence), cross-pass agreement, and coverage (how
//! many passes actually looked at the word), so fast typists whose words get
//! fewer looks are not starved of corrections.

use serde::Serialize;
use std::collections::BTreeMap;
use std::ops::Range;

/// One aligned difference between the draft and a candidate. `draft_range`
/// is a word-index range within the draft; a zero-length range is an
/// insertion before that word, and an empty `replacement` is a deletion.
#[derive(Debug, Clone, PartialEq)]
pub struct WordEdit {
    pub draft_range: Range<usize>,
    pub replacement: String,
}

/// Word-level LCS alignment of a candidate against the draft. Handles 1→N
/// expansions ("tmrw" → "see you tomorrow"), N→1 merges, insertions, and
/// deletions; a wholly-unaligned candidate degrades to one whole-draft edit.
pub fn diff_words(draft: &str, candidate: &str) -> Vec<WordEdit> {
    let a: Vec<&str> = draft.split_whitespace().collect();
    let b: Vec<&str> = candidate.split_whitespace().collect();
    let (n, m) = (a.len(), b.len());

    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut matches = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            matches.push((i, j));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }

    let mut edits = Vec::new();
    let (mut prev_a, mut prev_b) = (0, 0);
    for &(ma, mb) in matches.iter().chain(std::iter::once(&(n, m))) {
        if ma > prev_a || mb > prev_b {
            edits.push(WordEdit {
                draft_range: prev_a..ma,
                replacement: b[prev_b..mb].join(" "),
            });
        }
        prev_a = ma + 1;
        prev_b = mb + 1;
    }
    edits
}

/// Reapplies edits to a draft (word-normalized). Test/consolidation helper:
/// `apply_word_edits(draft, &diff_words(draft, c))` must reproduce `c` up to
/// whitespace.
pub fn apply_word_edits(draft: &str, edits: &[WordEdit]) -> String {
    let words: Vec<&str> = draft.split_whitespace().collect();
    let mut out: Vec<String> = Vec::new();
    let mut cursor = 0;
    for edit in edits {
        out.extend(
            words[cursor..edit.draft_range.start]
                .iter()
                .map(|w| w.to_string()),
        );
        if !edit.replacement.is_empty() {
            out.push(edit.replacement.clone());
        }
        cursor = edit.draft_range.end;
    }
    out.extend(words[cursor..].iter().map(|w| w.to_string()));
    out.join(" ")
}

/// A proposed correction surfaced to the UI, in session word coordinates.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Mark {
    /// Session word index of the first affected word.
    pub start_word: usize,
    /// Number of draft words replaced (0 = insertion before `start_word`).
    pub word_len: usize,
    /// The affected original words, joined (empty for an insertion).
    pub original: String,
    pub replacement: String,
    pub agreements: u32,
    /// True once the hardening rule fired: stable enough to act on.
    pub stable: bool,
}

#[derive(Debug, Clone)]
struct SlotState {
    word_len: usize,
    replacement: String,
    /// Passes in a row that proposed exactly this replacement.
    agreements: u32,
    /// Passes that covered this range at all (proposing or not).
    looks: u32,
    /// A pass proposed this with top-candidate votes ≥ 3 or a unanimous
    /// (single-candidate) result.
    strong: bool,
    /// votes ≥ 4 of 5, or unanimous: trustworthy even on a single look.
    very_strong: bool,
    hardened: bool,
}

/// The evidence a pass carries about its top candidate's confidence.
#[derive(Debug, Clone, Copy)]
pub struct PassSignal {
    /// Votes for the top candidate, when the producer voted.
    pub top_votes: Option<u32>,
    /// The raw samples deduplicated to a single candidate: unanimity.
    pub unanimous: bool,
}

impl PassSignal {
    fn strong(self) -> bool {
        self.unanimous || self.top_votes.is_some_and(|v| v >= 3)
    }
    fn very_strong(self) -> bool {
        self.unanimous || self.top_votes.is_some_and(|v| v >= 4)
    }
}

/// Session-scoped accumulator: word slots keyed by session word index.
/// Reset at every composition-session boundary.
#[derive(Default)]
pub struct SessionEdits {
    /// Original words observed so far, by session word index.
    words: Vec<Option<String>>,
    slots: BTreeMap<usize, SlotState>,
    /// One past the highest word index observed: the caret proxy used by the
    /// "caret has moved on" half of the hardening rule.
    frontier: usize,
}

impl SessionEdits {
    /// Feeds one settled pass: the burst at `word_offset` produced
    /// `top_candidate`. Records originals, updates slots, decays hypotheses
    /// the pass no longer proposes, and hardens slots that qualify.
    pub fn observe(
        &mut self,
        word_offset: usize,
        draft: &str,
        top_candidate: &str,
        signal: PassSignal,
    ) {
        let draft_words: Vec<&str> = draft.split_whitespace().collect();
        self.record_words(word_offset, &draft_words);
        let covered = word_offset..word_offset + draft_words.len();

        let mut proposed: Vec<usize> = Vec::new();
        for edit in diff_words(draft, top_candidate) {
            let start = word_offset + edit.draft_range.start;
            let word_len = edit.draft_range.len();
            proposed.push(start);
            if let Some(slot) = self.slots.get_mut(&start) {
                if slot.hardened {
                    // Hardened slots are settled; later passes don't reopen
                    // them (they were computed from the uncorrected text).
                    slot.looks += 1;
                    continue;
                }
                if slot.word_len == word_len && slot.replacement == edit.replacement {
                    slot.agreements += 1;
                    slot.looks += 1;
                    slot.strong |= signal.strong();
                    slot.very_strong |= signal.very_strong();
                    continue;
                }
            }
            // New proposal, or a different one than before: (re)start at one
            // agreement with the fresh hypothesis.
            self.slots.insert(
                start,
                SlotState {
                    word_len,
                    replacement: edit.replacement,
                    agreements: 1,
                    looks: 1,
                    strong: signal.strong(),
                    very_strong: signal.very_strong(),
                    hardened: false,
                },
            );
        }

        self.decay_unproposed(&covered, &proposed);
        self.harden_qualifying();
    }

    /// Feeds a pass that proposed no change for its window ("the text is
    /// fine" is evidence too): originals are recorded and covered hypotheses
    /// decay toward removal.
    pub fn observe_no_change(&mut self, word_offset: usize, draft: &str) {
        let draft_words: Vec<&str> = draft.split_whitespace().collect();
        self.record_words(word_offset, &draft_words);
        let covered = word_offset..word_offset + draft_words.len();
        self.decay_unproposed(&covered, &[]);
        self.harden_qualifying();
    }

    fn record_words(&mut self, offset: usize, draft_words: &[&str]) {
        let end = offset + draft_words.len();
        if self.words.len() < end {
            self.words.resize(end, None);
        }
        for (k, word) in draft_words.iter().enumerate() {
            self.words[offset + k] = Some(word.to_string());
        }
        self.frontier = self.frontier.max(end);
    }

    fn decay_unproposed(&mut self, covered: &Range<usize>, proposed: &[usize]) {
        let mut dead = Vec::new();
        for (&start, slot) in self.slots.iter_mut() {
            let end = start + slot.word_len.max(1);
            let inside = start >= covered.start && end <= covered.end;
            if inside && !slot.hardened && !proposed.contains(&start) {
                slot.looks += 1;
                slot.agreements = slot.agreements.saturating_sub(1);
                if slot.agreements == 0 {
                    dead.push(start);
                }
            }
        }
        for start in dead {
            self.slots.remove(&start);
        }
    }

    /// The hardening rule, coverage-adaptive: the caret must be ≥ 2 words
    /// past the slot, and then either three agreeing passes, two agreeing
    /// passes with a strong signal, or every look agreeing with a very
    /// strong signal (which lets a single-look pass — the chunked cadence —
    /// still harden on 4-of-5 votes or unanimity).
    fn harden_qualifying(&mut self) {
        for (&start, slot) in self.slots.iter_mut() {
            if slot.hardened {
                continue;
            }
            let end = start + slot.word_len;
            if self.frontier < end + 2 {
                continue;
            }
            let qualifies = slot.agreements >= 3
                || (slot.agreements >= 2 && slot.strong)
                || (slot.agreements >= slot.looks && slot.very_strong);
            if qualifies {
                slot.hardened = true;
            }
        }
    }

    /// Every current proposal, hardened or not, oldest first. The UI shows
    /// only `stable` marks; the rest are exposed for the stats line.
    pub fn marks(&self) -> Vec<Mark> {
        self.slots
            .iter()
            .map(|(&start, slot)| Mark {
                start_word: start,
                word_len: slot.word_len,
                original: self.original_span(start, slot.word_len),
                replacement: slot.replacement.clone(),
                agreements: slot.agreements,
                stable: slot.hardened,
            })
            .collect()
    }

    /// Removes and returns all hardened marks (the apply-all path), updating
    /// the session's word record as if the replacements were typed. Marks
    /// come back oldest-first with their pre-apply indices, so a caller
    /// applying them to real text must work right to left.
    pub fn take_hardened(&mut self) -> Vec<Mark> {
        let hardened: Vec<Mark> = self
            .marks()
            .into_iter()
            .filter(|mark| mark.stable)
            .collect();
        for mark in hardened.iter().rev() {
            self.splice_words(mark.start_word, mark.word_len, &mark.replacement);
        }
        hardened
    }

    /// The whole session with every hardened correction substituted, for the
    /// sentence-consolidation offer. None when nothing hardened or when the
    /// session has unobserved gaps.
    pub fn consolidated(&self) -> Option<String> {
        if !self.slots.values().any(|slot| slot.hardened) {
            return None;
        }
        let mut out: Vec<String> = Vec::new();
        let mut index = 0;
        while index < self.frontier {
            if let Some(slot) = self.slots.get(&index).filter(|s| s.hardened) {
                if !slot.replacement.is_empty() {
                    out.push(slot.replacement.clone());
                }
                if slot.word_len > 0 {
                    index += slot.word_len;
                    continue;
                }
                // An insertion keeps the word it was inserted before.
            }
            out.push(self.words.get(index).cloned().flatten()?);
            index += 1;
        }
        Some(out.join(" "))
    }

    /// An accepted candidate-bar commit replaced `old_len` words at
    /// `start_word` with `committed`: resolve overlapping slots and shift
    /// everything after the range so word indices stay true.
    pub fn shift_after_commit(&mut self, start_word: usize, old_len: usize, committed: &str) {
        self.splice_words(start_word, old_len, committed);
    }

    fn splice_words(&mut self, start: usize, old_len: usize, replacement: &str) {
        let replacement_words: Vec<String> =
            replacement.split_whitespace().map(str::to_string).collect();
        let new_len = replacement_words.len();
        let end = (start + old_len).min(self.words.len());
        self.words
            .splice(start..end, replacement_words.into_iter().map(Some));

        let delta = new_len as i64 - old_len as i64;
        let old_slots = std::mem::take(&mut self.slots);
        for (slot_start, slot) in old_slots {
            let slot_end = slot_start + slot.word_len.max(1);
            if slot_end <= start {
                self.slots.insert(slot_start, slot);
            } else if slot_start >= start + old_len {
                self.slots
                    .insert((slot_start as i64 + delta) as usize, slot);
            }
            // Slots overlapping the replaced range are resolved by the
            // commit and dropped.
        }
        self.frontier = (self.frontier as i64 + delta).max(0) as usize;
    }

    /// The number of words the session has observed (stats).
    pub fn word_count(&self) -> usize {
        self.frontier
    }

    /// The session's original words joined, when every word was observed.
    pub fn original_text(&self) -> Option<String> {
        (0..self.frontier)
            .map(|i| self.words.get(i).cloned().flatten())
            .collect::<Option<Vec<_>>>()
            .map(|words| words.join(" "))
    }

    fn original_span(&self, start: usize, len: usize) -> String {
        (start..start + len)
            .filter_map(|i| self.words.get(i).cloned().flatten())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal_plain() -> PassSignal {
        PassSignal {
            top_votes: None,
            unanimous: false,
        }
    }

    fn signal_unanimous() -> PassSignal {
        PassSignal {
            top_votes: Some(5),
            unanimous: true,
        }
    }

    #[test]
    fn diff_aligns_replacements_expansions_and_insertions() {
        // Wholly unaligned: one whole-draft edit.
        let edits = diff_words("cnt cm tmrw", "Can't come tomorrow.");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].draft_range, 0..3);

        // Partial alignment: two isolated word fixes.
        let edits = diff_words(
            "i went to the store instaed",
            "I went to the store instead.",
        );
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].draft_range, 0..1);
        assert_eq!(edits[0].replacement, "I");
        assert_eq!(edits[1].draft_range, 5..6);
        assert_eq!(edits[1].replacement, "instead.");

        // 1→N expansion around a matched word.
        let edits = diff_words("brb 5 min", "Be right back in 5 minutes.");
        assert_eq!(edits[0].replacement, "Be right back in");
        assert_eq!(edits[1].draft_range, 2..3);

        // Insertion between matched words.
        let edits = diff_words("meet there tmrw", "meet you there tomorrow");
        assert_eq!(edits[0].draft_range, 1..1);
        assert_eq!(edits[0].replacement, "you");
    }

    #[test]
    fn diff_roundtrips_arbitrary_pairs() {
        let pairs = [
            ("cnt cm tmrw", "Can't come tomorrow."),
            (
                "i went to the store instaed",
                "I went to the store instead.",
            ),
            ("brb 5 min", "Be right back in 5 minutes."),
            ("thx so much 4 the help", "Thanks so much for the help."),
            ("omw", "On my way."),
            (
                "meet there tmrw",
                "Meet at Union Station at 8:30 AM tomorrow.",
            ),
        ];
        for (draft, candidate) in pairs {
            let rebuilt = apply_word_edits(draft, &diff_words(draft, candidate));
            let normalized = candidate.split_whitespace().collect::<Vec<_>>().join(" ");
            assert_eq!(rebuilt, normalized, "pair ({draft:?}, {candidate:?})");
        }
    }

    #[test]
    fn agreement_accumulates_and_hardens_behind_the_caret() {
        let mut session = SessionEdits::default();
        // Sliding passes keep proposing tmrw → tomorrow.
        session.observe(0, "cnt cm tmrw", "cnt cm tomorrow", signal_plain());
        session.observe(1, "cm tmrw ok", "cm tomorrow ok", signal_plain());
        assert!(session.marks().iter().all(|m| !m.stable), "caret too close");
        // Third agreement and the caret two words past: hardened.
        session.observe(2, "tmrw ok going", "tomorrow ok going", signal_plain());
        let marks = session.marks();
        let mark = marks.iter().find(|m| m.start_word == 2).unwrap();
        assert!(mark.stable);
        assert_eq!(mark.original, "tmrw");
        assert_eq!(mark.replacement, "tomorrow");
    }

    #[test]
    fn single_look_hardens_only_on_a_very_strong_signal() {
        let mut session = SessionEdits::default();
        // Chunked cadence: each word gets one look. Plain signal never
        // hardens; unanimity does once the caret moves on.
        session.observe(0, "cnt cm tmrw", "Can't come tomorrow.", signal_plain());
        session.observe(3, "ok going now", "ok going now.", signal_plain());
        assert!(session.marks().iter().all(|m| !m.stable));

        let mut session = SessionEdits::default();
        session.observe(0, "cnt cm tmrw", "Can't come tomorrow.", signal_unanimous());
        session.observe(3, "ok going now", "ok going now.", signal_plain());
        let marks = session.marks();
        assert!(marks.iter().any(|m| m.stable && m.start_word == 0));
    }

    #[test]
    fn changed_hypothesis_resets_and_no_change_decays() {
        let mut session = SessionEdits::default();
        session.observe(0, "cnt cm tmrw", "cnt cm tomorrow", signal_plain());
        // A different proposal replaces the hypothesis at agreement 1.
        session.observe(0, "cnt cm tmrw", "cnt cm tmw", signal_plain());
        let mark = &session.marks()[0];
        assert_eq!(mark.replacement, "tmw");
        assert_eq!(mark.agreements, 1);
        // A no-change pass decays the single agreement away entirely.
        session.observe_no_change(0, "cnt cm tmrw");
        assert!(session.marks().is_empty());
    }

    #[test]
    fn take_hardened_returns_marks_and_consolidation_stitches() {
        let mut session = SessionEdits::default();
        session.observe(0, "cnt cm tmrw", "Can't come tomorrow.", signal_unanimous());
        session.observe(3, "ok going now", "ok going now", signal_plain());
        assert_eq!(
            session.consolidated().as_deref(),
            Some("Can't come tomorrow. ok going now")
        );
        let taken = session.take_hardened();
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].replacement, "Can't come tomorrow.");
        // Applied: no hardened marks remain, and the word record now holds
        // the corrected words.
        assert!(session.take_hardened().is_empty());
        assert_eq!(session.consolidated(), None);
        assert_eq!(session.word_count(), 6); // 3 words replaced 3, plus 3 more
    }

    #[test]
    fn commits_shift_downstream_slots() {
        let mut session = SessionEdits::default();
        session.observe(0, "cnt cm tmrw", "cnt cm tomorrow", signal_plain());
        session.observe(3, "c u thr", "see you there", signal_unanimous());
        // A bar commit replaced words 0..3 with four words: +1 delta.
        session.shift_after_commit(0, 3, "Can't come tomorrow, ok.");
        let marks = session.marks();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].start_word, 4);
        assert_eq!(marks[0].original, "c u thr");
        let _ = session.consolidated();
    }
}
