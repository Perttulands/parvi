//! Rows picked out of the Sessions rail to kill together, and the kills in
//! flight, as CHROTE's `sessionSelection.ts` and `deleteSessions` (#184).
//!
//! The pick lives on this device only and is never kept. Its keys are
//! `Key::text()`, so two Unix users' sessions of one name are two rows. The
//! anchor is the last row clicked, where the next Shift+click range starts.
//! The pick may hold rows the rail does not show now (searched away, in a
//! collapsed group): they stay picked, but only rows shown are ever killed.

use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pick {
    pub keys: HashSet<String>,
    pub anchor: Option<String>,
}

impl Pick {
    pub fn has(&self, key: &str) -> bool {
        self.keys.contains(key)
    }

    /// Ctrl+click: the row flips in or out.
    pub fn toggle(&mut self, key: &str) {
        if !self.keys.remove(key) {
            self.keys.insert(key.to_string());
        }
        self.anchor = Some(key.to_string());
    }

    /// Shift+click: every shown row from the last clicked one to this one
    /// joins. Without an anchor that is shown, only this row joins.
    pub fn range(&mut self, key: &str, shown: &[String]) {
        let to = shown.iter().position(|k| k == key);
        let from = (self.anchor.as_ref()).and_then(|a| shown.iter().position(|k| k == a));
        match (from, to) {
            (Some(from), Some(to)) => {
                let (a, b) = if from <= to { (from, to) } else { (to, from) };
                self.keys.extend(shown[a..=b].iter().cloned());
            }
            _ => {
                self.keys.insert(key.to_string());
            }
        }
        self.anchor = Some(key.to_string());
    }

    /// A plain click empties the pick; the row is where a range would start.
    pub fn plain(&mut self, key: &str) {
        self.keys.clear();
        self.anchor = Some(key.to_string());
    }

    pub fn clear(&mut self) {
        self.keys.clear();
    }

    /// Sessions that are gone leave the pick, so a later session that takes
    /// the name is not killed for its predecessor.
    pub fn retain_existing(&mut self, exists: impl Fn(&str) -> bool) {
        self.keys.retain(|k| exists(k));
    }

    /// The picked rows that are shown, in the order shown.
    pub fn targets(&self, shown: &[String]) -> Vec<String> {
        shown
            .iter()
            .filter(|k| self.keys.contains(*k))
            .cloned()
            .collect()
    }

    /// Killed rows leave; rows that failed, and rows not shown, stay.
    pub fn without(&mut self, killed: &[String]) {
        for k in killed {
            self.keys.remove(k);
        }
    }
}

/// One kill asked of a node: which request, and of which row.
pub struct Sent {
    pub node: String,
    pub id: u32,
    pub key: String,
    pub name: String,
}

/// How a kill went, once every session in it has answered.
#[derive(Debug, PartialEq)]
pub struct Receipt {
    pub text: String,
    pub error: bool,
    /// The rows killed, as `Key::text()`.
    pub killed: Vec<String>,
}

struct Batch {
    /// Several rows picked and killed together: one receipt counts them.
    picked: bool,
    total: usize,
    waiting: usize,
    killed: Vec<String>,
    /// Name and why, in the order answered.
    failed: Vec<(String, String)>,
}

impl Batch {
    fn receipt(&mut self) -> Receipt {
        let killed = std::mem::take(&mut self.killed);
        let (text, error) = match (self.picked, self.failed.as_slice()) {
            (false, []) => (format!("Killed {}", self.names_killed(&killed)), false),
            (false, [(name, why)]) => (format!("Could not kill {name}: {why}"), true),
            (true, []) => (format!("Killed {}", count(killed.len())), false),
            (_, failed) => (
                format!(
                    "Killed {} of {} — failed: {}",
                    killed.len(),
                    self.total,
                    failed
                        .iter()
                        .map(|(name, why)| format!("{name} ({why})"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                true,
            ),
        };
        Receipt {
            text,
            error,
            killed,
        }
    }

    fn names_killed(&self, killed: &[String]) -> String {
        // A key is `node:user:name`; a name holds no `:`.
        (killed.iter())
            .map(|k| k.rsplit(':').next().unwrap_or(k))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn count(n: usize) -> String {
    if n == 1 {
        "1 session".into()
    } else {
        format!("{n} sessions")
    }
}

/// The kills in flight, each waiting for its node's answer.
#[derive(Default)]
pub struct Kills {
    batches: Vec<Batch>,
    /// (node, request id) to its batch and row.
    pending: HashMap<(String, u32), (usize, String, String)>,
}

impl Kills {
    /// Starts a kill of `sent`, with `refused` (name, why) not asked at all.
    /// Answers at once if nothing was asked.
    pub fn start(
        &mut self,
        picked: bool,
        sent: Vec<Sent>,
        refused: Vec<(String, String)>,
    ) -> Option<Receipt> {
        let mut batch = Batch {
            picked,
            total: sent.len() + refused.len(),
            waiting: sent.len(),
            killed: vec![],
            failed: refused,
        };
        if sent.is_empty() {
            return (batch.total > 0).then(|| batch.receipt());
        }
        let at = self.batches.len();
        for s in sent {
            self.pending.insert((s.node, s.id), (at, s.key, s.name));
        }
        self.batches.push(batch);
        None
    }

    /// A node answered one kill: the receipt once its batch is complete.
    pub fn answered(&mut self, node: &str, id: u32, ok: bool, why: &str) -> Option<Receipt> {
        let (at, key, name) = self.pending.remove(&(node.to_string(), id))?;
        let batch = &mut self.batches[at];
        batch.waiting -= 1;
        if ok {
            batch.killed.push(key);
        } else {
            let why = if why.is_empty() {
                "the node refused"
            } else {
                why
            };
            batch.failed.push((name, why.to_string()));
        }
        if batch.waiting > 0 {
            return None;
        }
        let receipt = batch.receipt();
        if self.pending.is_empty() {
            self.batches.clear();
        }
        Some(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown() -> Vec<String> {
        ["a", "b", "c", "d", "e"].map(String::from).to_vec()
    }

    fn keys(p: &Pick) -> Vec<&str> {
        let mut k: Vec<&str> = p.keys.iter().map(String::as_str).collect();
        k.sort();
        k
    }

    // CHROTE's sessionSelection.test.ts, case by case.

    #[test]
    fn toggles_one_row_in_and_out_with_ctrl() {
        let mut p = Pick::default();
        p.toggle("b");
        p.toggle("d");
        assert_eq!(keys(&p), ["b", "d"]);
        p.toggle("b");
        assert_eq!(keys(&p), ["d"]);
    }

    #[test]
    fn adds_the_shown_rows_between_the_last_clicked_and_the_shift_clicked_either_way() {
        let mut p = Pick::default();
        p.toggle("b");
        p.range("d", &shown());
        assert_eq!(keys(&p), ["b", "c", "d"]);
        let mut p = Pick::default();
        p.toggle("e");
        p.range("c", &shown());
        assert_eq!(keys(&p), ["c", "d", "e"]);
        // A range starts where the last click landed, plain or not.
        let mut p = Pick::default();
        p.plain("a");
        p.range("b", &shown());
        assert_eq!(keys(&p), ["a", "b"]);
    }

    #[test]
    fn picks_only_the_clicked_row_when_the_anchor_is_not_shown() {
        let mut p = Pick::default();
        p.toggle("z");
        p.range("c", &shown());
        assert_eq!(keys(&p), ["c", "z"]);
    }

    #[test]
    fn a_plain_click_empties_the_pick() {
        let mut p = Pick::default();
        p.toggle("a");
        p.plain("d");
        assert!(p.keys.is_empty());
        assert_eq!(p.anchor.as_deref(), Some("d"));
    }

    #[test]
    fn kills_only_the_picked_rows_shown_in_the_order_shown() {
        let p = Pick {
            keys: ["d", "hidden", "b"].map(String::from).into(),
            anchor: Some("b".into()),
        };
        assert_eq!(p.targets(&shown()), ["b", "d"]);
        assert!(Pick::default().targets(&shown()).is_empty());
    }

    #[test]
    fn drops_a_session_that_is_gone_so_a_new_one_under_its_name_is_not_picked() {
        let mut p = Pick {
            keys: ["a", "gone"].map(String::from).into(),
            anchor: Some("a".into()),
        };
        p.retain_existing(|k| k == "a" || k == "b");
        assert_eq!(keys(&p), ["a"]);
    }

    #[test]
    fn keeps_failed_and_hidden_rows_picked_after_a_kill() {
        let mut p = Pick {
            keys: ["a", "b", "hidden"].map(String::from).into(),
            anchor: Some("a".into()),
        };
        p.without(&["a".into()]);
        assert_eq!(keys(&p), ["b", "hidden"]);
    }

    fn sent(node: &str, id: u32, name: &str) -> Sent {
        Sent {
            node: node.into(),
            id,
            key: format!("{node}::{name}"),
            name: name.into(),
        }
    }

    #[test]
    fn one_receipt_for_a_picked_kill_naming_each_failure() {
        let mut kills = Kills::default();
        let started = kills.start(
            true,
            vec![sent("n1", 1, "a"), sent("n2", 1, "b"), sent("n1", 2, "c")],
            vec![("d".into(), "its device is away".into())],
        );
        assert_eq!(started, None);
        assert_eq!(kills.answered("n1", 1, true, "a"), None);
        assert_eq!(kills.answered("n2", 1, false, "no session"), None);
        assert_eq!(kills.answered("n9", 2, true, ""), None, "not ours");
        let receipt = kills.answered("n1", 2, true, "c").unwrap();
        assert_eq!(
            receipt,
            Receipt {
                text: "Killed 2 of 4 — failed: d (its device is away), b (no session)".into(),
                error: true,
                killed: vec!["n1::a".into(), "n1::c".into()],
            }
        );
        assert_eq!(kills.answered("n1", 2, true, "c"), None, "answered once");
    }

    #[test]
    fn a_picked_kill_that_all_went_counts_them() {
        let mut kills = Kills::default();
        kills.start(true, vec![sent("n", 1, "a"), sent("n", 2, "b")], vec![]);
        kills.answered("n", 2, true, "");
        let receipt = kills.answered("n", 1, true, "").unwrap();
        assert_eq!(
            (receipt.text.as_str(), receipt.error),
            ("Killed 2 sessions", false)
        );
        let mut kills = Kills::default();
        kills.start(true, vec![sent("n", 1, "a")], vec![]);
        assert_eq!(
            kills.answered("n", 1, true, "").unwrap().text,
            "Killed 1 session"
        );
    }

    #[test]
    fn a_single_kill_names_its_session() {
        let mut kills = Kills::default();
        kills.start(false, vec![sent("n", 1, "a")], vec![]);
        assert_eq!(kills.answered("n", 1, true, "").unwrap().text, "Killed a");
        kills.start(false, vec![sent("n", 2, "b")], vec![]);
        let receipt = kills.answered("n", 2, false, "").unwrap();
        assert_eq!(
            (receipt.text.as_str(), receipt.error),
            ("Could not kill b: the node refused", true)
        );
        let away = kills
            .start(
                false,
                vec![],
                vec![("c".into(), "its device is away".into())],
            )
            .unwrap();
        assert_eq!(away.text, "Could not kill c: its device is away");
    }
}
