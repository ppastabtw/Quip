//! Workstream 4: local per-user learning.
//!
//! Profile-scoped storage in the app data dir: append-only compact labeled
//! examples (`profiles/<id>/examples.jsonl`) and the deduplicated pattern
//! dictionary (`profiles/<id>/patterns.json`) that feeds `personal_patterns`
//! in prediction requests. Personal records never leave the Mac and are never
//! committed to git. Plain JSON on purpose: "inspect stored patterns" is a
//! product feature.

use quip_contracts::{ModelVariant, PersonalPattern};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Patterns must repeat before they are trusted enough to send with requests.
const PATTERN_USE_THRESHOLD: u32 = 2;
/// Requests stay compact: only the strongest patterns are attached.
const PATTERNS_PER_REQUEST: usize = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatternEntry {
    pub expansion: String,
    pub count: u32,
}

/// One compact labeled interaction appended to `examples.jsonl`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabeledExample {
    pub ts_ms: u64,
    pub burst_id: String,
    pub profile_id: String,
    pub draft: String,
    /// Internal learning label such as `keep` or `replace`.
    pub label: String,
    pub committed: String,
    /// Interaction source such as `confirmed_candidate` or `dismissal`.
    pub source: String,
    pub model_variant: ModelVariant,
}

pub struct LearningStore {
    root: PathBuf,
}

impl LearningStore {
    /// Opens the store rooted at `<data_dir>/profiles`, seeding the two demo
    /// profiles (judged-build requirement: two local profiles that produce
    /// different candidates) plus an empty default profile on first run.
    pub fn open(data_dir: &Path) -> Self {
        let store = Self {
            root: data_dir.join("profiles"),
        };
        store.seed_if_missing("profile_default", &[]);
        store.seed_if_missing("profile_a", &[("tn", "tonight"), ("eod", "end of day")]);
        store.seed_if_missing(
            "profile_b",
            &[("tn", "tomorrow night"), ("eod", "end of demo")],
        );
        store
    }

    fn profile_dir(&self, profile_id: &str) -> PathBuf {
        self.root.join(sanitize(profile_id))
    }

    fn patterns_path(&self, profile_id: &str) -> PathBuf {
        self.profile_dir(profile_id).join("patterns.json")
    }

    fn seed_if_missing(&self, profile_id: &str, pairs: &[(&str, &str)]) {
        let path = self.patterns_path(profile_id);
        if path.exists() {
            return;
        }
        let patterns: BTreeMap<String, PatternEntry> = pairs
            .iter()
            .map(|(s, e)| {
                (
                    s.to_string(),
                    PatternEntry {
                        expansion: e.to_string(),
                        // Seeds start above the trust threshold so demo
                        // profiles personalize immediately.
                        count: PATTERN_USE_THRESHOLD + 1,
                    },
                )
            })
            .collect();
        self.save_patterns(profile_id, &patterns);
    }

    pub fn list_profiles(&self) -> Vec<String> {
        let mut profiles: Vec<String> = std::fs::read_dir(&self.root)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default();
        profiles.sort();
        profiles
    }

    pub fn load_patterns(&self, profile_id: &str) -> BTreeMap<String, PatternEntry> {
        std::fs::read_to_string(self.patterns_path(profile_id))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn save_patterns(&self, profile_id: &str, patterns: &BTreeMap<String, PatternEntry>) {
        let dir = self.profile_dir(profile_id);
        if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| {
            std::fs::write(
                self.patterns_path(profile_id),
                serde_json::to_string_pretty(patterns).unwrap(),
            )
        }) {
            tracing::warn!(error = %e, profile_id, "failed to save patterns");
        }
    }

    /// Records one observed shorthand → expansion pair. Repeats of the same
    /// pair increment its count; a conflicting expansion replaces the entry
    /// and restarts trust from one.
    pub fn record_pattern(&self, profile_id: &str, shorthand: &str, expansion: &str) {
        let mut patterns = self.load_patterns(profile_id);
        match patterns.get_mut(shorthand) {
            Some(entry) if entry.expansion == expansion => entry.count += 1,
            _ => {
                patterns.insert(
                    shorthand.to_string(),
                    PatternEntry {
                        expansion: expansion.to_string(),
                        count: 1,
                    },
                );
            }
        }
        self.save_patterns(profile_id, &patterns);
    }

    /// The compact pattern list attached to prediction requests: trusted
    /// patterns only, strongest first, bounded count.
    pub fn patterns_for_request(&self, profile_id: &str) -> Vec<PersonalPattern> {
        let mut entries: Vec<(String, PatternEntry)> = self
            .load_patterns(profile_id)
            .into_iter()
            .filter(|(_, e)| e.count >= PATTERN_USE_THRESHOLD)
            .collect();
        entries.sort_by(|(sa, ea), (sb, eb)| eb.count.cmp(&ea.count).then(sa.cmp(sb)));
        entries
            .into_iter()
            .take(PATTERNS_PER_REQUEST)
            .map(|(shorthand, e)| PersonalPattern {
                shorthand,
                expansion: e.expansion,
            })
            .collect()
    }

    pub fn append_example(&self, example: &LabeledExample) {
        let dir = self.profile_dir(&example.profile_id);
        let path = dir.join("examples.jsonl");
        let result = std::fs::create_dir_all(&dir).and_then(|_| {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?;
            writeln!(file, "{}", serde_json::to_string(example).unwrap())
        });
        if let Err(e) = result {
            tracing::warn!(error = %e, path = %path.display(), "failed to append example");
        }
    }

    /// Deletes a profile's examples and learned patterns (the product's
    /// "reset my local records" control), then reseeds demo defaults.
    pub fn reset_profile(&self, profile_id: &str) {
        let dir = self.profile_dir(profile_id);
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(error = %e, profile_id, "failed to reset profile");
            }
        }
        match profile_id {
            "profile_a" => {
                self.seed_if_missing(profile_id, &[("tn", "tonight"), ("eod", "end of day")])
            }
            "profile_b" => self.seed_if_missing(
                profile_id,
                &[("tn", "tomorrow night"), ("eod", "end of demo")],
            ),
            _ => self.seed_if_missing(profile_id, &[]),
        }
    }
}

/// Mines shorthand → expansion candidates from a confirmed replacement by
/// aligning tokens positionally. Only same-length bursts are mined; anything
/// fancier belongs to the local trainer, not the dictionary.
pub fn extract_patterns(draft: &str, committed: &str) -> Vec<(String, String)> {
    let draft_tokens: Vec<&str> = draft.split_whitespace().collect();
    let committed_tokens: Vec<&str> = committed.split_whitespace().collect();
    if draft_tokens.len() != committed_tokens.len() {
        return Vec::new();
    }
    draft_tokens
        .iter()
        .zip(committed_tokens.iter())
        .filter_map(|(d, c)| {
            let d = d.trim_matches(TOKEN_PUNCT).to_lowercase();
            let c = c.trim_matches(TOKEN_PUNCT);
            if d.len() >= 2
                && !c.is_empty()
                && d.chars().all(|ch| ch.is_alphanumeric())
                && d != c.to_lowercase()
            {
                Some((d, c.to_string()))
            } else {
                None
            }
        })
        .collect()
}

const TOKEN_PUNCT: &[char] = &['.', ',', '!', '?', ';', ':'];

fn sanitize(profile_id: &str) -> String {
    profile_id
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect()
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (LearningStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "quip-learning-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        (LearningStore::open(&dir), dir)
    }

    #[test]
    fn seeds_demo_profiles_with_diverging_patterns() {
        let (store, dir) = temp_store();
        assert_eq!(
            store.list_profiles(),
            vec!["profile_a", "profile_b", "profile_default"]
        );
        let a = store.patterns_for_request("profile_a");
        let b = store.patterns_for_request("profile_b");
        let a_tn = a.iter().find(|p| p.shorthand == "tn").unwrap();
        let b_tn = b.iter().find(|p| p.shorthand == "tn").unwrap();
        assert_ne!(a_tn.expansion, b_tn.expansion);
        assert!(store.patterns_for_request("profile_default").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn repeated_patterns_deduplicate_and_cross_threshold() {
        let (store, dir) = temp_store();
        store.record_pattern("profile_default", "brb", "be right back");
        assert!(store.patterns_for_request("profile_default").is_empty());
        store.record_pattern("profile_default", "brb", "be right back");
        let patterns = store.patterns_for_request("profile_default");
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].expansion, "be right back");
        assert_eq!(store.load_patterns("profile_default").len(), 1);

        // A conflicting expansion replaces the entry and loses trust.
        store.record_pattern("profile_default", "brb", "bring rye bread");
        assert!(store.patterns_for_request("profile_default").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn extracts_aligned_token_changes_only() {
        assert_eq!(
            extract_patterns("ship spec tn", "Ship spec tonight."),
            vec![("tn".to_string(), "tonight".to_string())]
        );
        // Different token counts: no mining.
        assert!(extract_patterns("cnt cm tmrw", "I can't come tomorrow.").is_empty());
        // Single-character and non-alphanumeric draft tokens are skipped.
        assert!(extract_patterns("a b/c", "at b/d").is_empty());
    }

    #[test]
    fn reset_clears_examples_and_reseeds() {
        let (store, dir) = temp_store();
        store.append_example(&LabeledExample {
            ts_ms: 1,
            burst_id: "b1".into(),
            profile_id: "profile_a".into(),
            draft: "x".into(),
            label: "keep".into(),
            committed: "x".into(),
            source: "exact_draft".into(),
            model_variant: quip_contracts::ModelVariant::Global,
        });
        store.reset_profile("profile_a");
        assert!(!dir.join("profiles/profile_a/examples.jsonl").exists());
        assert!(!store.patterns_for_request("profile_a").is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
