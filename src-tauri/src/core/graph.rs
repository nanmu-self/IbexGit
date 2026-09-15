//! Commit graph pipeline (PLAN P5): GraphLayout — lane assignment over a
//! topo-ordered commit stream.
//!
//! Pipeline layers (PLAN §P5): GraphQuery (engine log) → GraphCache
//! (repo::Session) → GraphLayout (this module) → GraphRenderer (frontend
//! SVG). The layout is a pure, resumable function: it consumes commits in
//! topo-order (children before parents) and maintains an "active lanes"
//! state so batches can be appended incrementally (500/批 分批加载).
//!
//! Row model — every row is self-contained so virtualized rendering never
//! needs neighbouring rows:
//!
//! - an edge "flows" down through lanes; `active[l]` is the commit that
//!   the flow in lane `l` currently points at (it will occupy that lane
//!   when laid out);
//! - each row decomposes every crossing edge into at most two halves:
//!   upper half = into this row's node (`to_node`), lower half = out of
//!   this row's node (`from_node`), or a full vertical pass-through;
//! - lane indices never shift once assigned; released lanes go to a
//!   free-list and are reused lowest-first (gitk behaviour), keeping the
//!   column count close to the actual branch concurrency.

use crate::core::engine::CommitInfo;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

// =====================
// Wire types (specta → TS)
// =====================

/// One edge segment drawn within a single commit row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct GraphEdge {
    /// Lane (column) where the edge starts on this row.
    pub from: u32,
    /// Lane (column) where the edge ends on this row.
    pub to: u32,
    /// The edge starts at this row's node (mid-height, lower half).
    pub from_node: bool,
    /// The edge ends at this row's node (mid-height, upper half).
    pub to_node: bool,
}

/// One rendered row of the commit graph: the commit plus its lane and the
/// edge segments crossing the row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct GraphRow {
    pub commit: CommitInfo,
    /// Column of this commit's node.
    pub lane: u32,
    pub edges: Vec<GraphEdge>,
}

/// One page of the commit graph served by the GraphCache (P5). `start` is
/// the row index of `rows[0]` within the full graph: `0` means the cache
/// was rebuilt and the frontend must replace its list, otherwise the rows
/// append after the previously loaded ones.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct GraphPage {
    pub rows: Vec<GraphRow>,
    pub start: u32,
    /// `git log` exhausted — no further pages.
    pub complete: bool,
    /// Lanes used so far (SVG column count).
    pub width: u32,
}

// =====================
// Layout
// =====================

/// Resumable lane state (GraphCache 的布局续算单元).
#[derive(Debug, Default, Clone)]
pub struct LayoutState {
    /// `active[l]` = commit hash the flow in lane `l` points at.
    active: Vec<Option<String>>,
    /// Released lanes available for reuse (lowest first).
    free: BTreeSet<usize>,
    /// Flowing commit → its lane (O(1) lookups keep the 100k-commit
    /// layout inside the tens-of-milliseconds target).
    lane_of: HashMap<String, usize>,
    /// Highest lane index ever used + 1 (the SVG column count).
    pub width: usize,
}

impl LayoutState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of currently active (non-free) lanes.
    pub fn active_count(&self) -> usize {
        self.active.iter().filter(|s| s.is_some()).count()
    }

    /// Number of released lanes available for reuse (test/debug aid).
    pub fn free_len(&self) -> usize {
        self.free.len()
    }

    /// Lay out `commits` (topo-order, children first), appending rendered
    /// rows to `out`. Resumable: call again with the next batch using the
    /// same state (分批增量加载).
    pub fn layout(&mut self, commits: &[CommitInfo], out: &mut Vec<GraphRow>) {
        for commit in commits {
            let (lane, edges) = self.layout_one(commit);
            out.push(GraphRow {
                commit: commit.clone(),
                lane: lane as u32,
                edges,
            });
        }
    }

    /// Lay out a single commit; returns its lane and edge segments.
    fn layout_one(&mut self, commit: &CommitInfo) -> (usize, Vec<GraphEdge>) {
        let mut edges = Vec::new();

        // 1. This commit's lane: an existing lane already flowing towards
        //    it (a child routed a parent edge here), else the lowest free
        //    lane, else a brand-new column. At most one lane can point at
        //    a commit (flow_to reuses existing lanes), so lane_of has a
        //    single entry.
        let (lane, inflow) = match self.lane_of.remove(&commit.hash) {
            Some(l) => (l, true),
            None => (self.acquire_lane(), false),
        };
        if inflow {
            // Upper half: the flow arrives from the top into the node.
            edges.push(GraphEdge {
                from: lane as u32,
                to: lane as u32,
                from_node: false,
                to_node: true,
            });
        }

        // 2. Route parents (lower-half edges out of the node). The first
        //    parent continues in the node's own lane when it has no lane
        //    yet (the common linear case draws as a straight vertical
        //    line); parents that already flow in some lane get a diagonal
        //    merge-in edge; remaining merge parents fan out to free lanes.
        let mut node_lane_reusable = true;
        for (i, parent) in commit.parents.iter().enumerate() {
            if let Some(&target) = self.lane_of.get(parent) {
                edges.push(GraphEdge {
                    from: lane as u32,
                    to: target as u32,
                    from_node: true,
                    to_node: false,
                });
                continue;
            }
            let target = if i == 0 && node_lane_reusable {
                node_lane_reusable = false;
                lane
            } else {
                self.acquire_lane()
            };
            self.flow_to(target, parent);
            edges.push(GraphEdge {
                from: lane as u32,
                to: target as u32,
                from_node: true,
                to_node: false,
            });
        }

        // 3. Release the node's lane unless the first parent took it over
        //    (then it stays active, already re-registered by flow_to).
        if node_lane_reusable {
            self.release_lane(lane);
        }

        // 4. Pass-through edges: every other flowing lane crosses this row
        //    vertically (upper half continues the previous row's bottom,
        //    lower half continues into the next row's top).
        for (l, slot) in self.active.iter().enumerate() {
            if slot.is_some() && l != lane {
                edges.push(GraphEdge {
                    from: l as u32,
                    to: l as u32,
                    from_node: false,
                    to_node: false,
                });
            }
        }

        (lane, edges)
    }

    /// Lowest free lane, else a new column.
    fn acquire_lane(&mut self) -> usize {
        match self.free.iter().next().copied() {
            Some(l) => {
                self.free.remove(&l);
                l
            }
            None => {
                let l = self.active.len();
                self.active.push(None);
                self.width = self.width.max(l + 1);
                l
            }
        }
    }

    fn release_lane(&mut self, l: usize) {
        if let Some(prev) = self.active.get(l).cloned().flatten() {
            self.lane_of.remove(&prev);
        }
        if l < self.active.len() {
            self.active[l] = None;
        }
        self.free.insert(l);
    }

    /// Point lane `l` at commit `hash` (the edge now flows towards it).
    fn flow_to(&mut self, l: usize, hash: &str) {
        if l >= self.active.len() {
            self.active.resize(l + 1, None);
            self.width = self.width.max(l + 1);
        }
        self.active[l] = Some(hash.to_string());
        self.lane_of.insert(hash.to_string(), l);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(hash: &str, parents: &[&str]) -> CommitInfo {
        CommitInfo {
            hash: hash.to_string(),
            short_hash: hash.to_string(),
            author: "a".into(),
            email: "a@t".into(),
            date: "2026-01-01T00:00:00Z".into(),
            message: hash.to_string(),
            refs: Vec::new(),
            parents: parents.iter().map(|p| p.to_string()).collect(),
        }
    }

    fn layout_all(commits: &[CommitInfo]) -> (Vec<GraphRow>, LayoutState) {
        let mut state = LayoutState::new();
        let mut rows = Vec::new();
        state.layout(commits, &mut rows);
        (rows, state)
    }

    fn edges_flag(row: &GraphRow, from_node: bool, to_node: bool) -> Vec<(u32, u32)> {
        row.edges
            .iter()
            .filter(|e| e.from_node == from_node && e.to_node == to_node)
            .map(|e| (e.from, e.to))
            .collect()
    }

    #[test]
    fn linear_history_uses_single_lane() {
        let (rows, state) =
            layout_all(&[commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])]);
        assert_eq!(state.width, 1);
        for (i, r) in rows.iter().enumerate() {
            assert_eq!(r.lane, 0);
            let out = edges_flag(r, true, false);
            let inflow = edges_flag(r, false, true);
            if i == 0 {
                // HEAD: no child above → no inflow edge.
                assert!(inflow.is_empty());
                assert_eq!(out, vec![(0, 0)]);
            } else if r.commit.parents.is_empty() {
                // Root: the flow from above ends here — inflow, no out.
                assert_eq!(inflow, vec![(0, 0)]);
                assert!(out.is_empty());
            } else {
                assert_eq!(out, vec![(0, 0)]);
                assert_eq!(inflow, vec![(0, 0)]);
            }
        }
    }

    #[test]
    fn branch_reuses_parent_lane_and_merges_back() {
        // main: a→b→c ; feature forks at b: d→(b)
        let (rows, state) = layout_all(&[
            commit("c", &["b"]),
            commit("d", &["b"]),
            commit("b", &["a"]),
            commit("a", &[]),
        ]);
        assert_eq!(state.width, 2, "fork needs a second column only");
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[1].lane, 1);
        // d's edge merges diagonally into b's lane (0), landing at the row
        // boundary (b's upper half continues it straight into the node).
        assert_eq!(edges_flag(&rows[1], true, false), vec![(1, 0)]);
        // b receives the straight inflow from c's lane.
        assert_eq!(edges_flag(&rows[2], false, true), vec![(0, 0)]);
        // Everything collapses back to a single lane at the root.
        assert_eq!(state.active_count(), 0);
    }

    #[test]
    fn merge_commit_fans_out_two_parents() {
        // M merges X and Y; both continue to Z.
        let (rows, state) = layout_all(&[
            commit("m", &["x", "y"]),
            commit("x", &["z"]),
            commit("y", &["z"]),
            commit("z", &[]),
        ]);
        assert_eq!(
            edges_flag(&rows[0], true, false),
            vec![(0, 0), (0, 1)],
            "first parent straight, second fans out"
        );
        // y merges diagonally into z's lane.
        assert_eq!(edges_flag(&rows[2], true, false), vec![(1, 0)]);
        assert_eq!(rows[3].lane, 0);
        assert_eq!(state.active_count(), 0);
        assert_eq!(state.width, 2);
    }

    #[test]
    fn first_parent_flowing_elsewhere_releases_node_lane() {
        // m's first parent x already flows in lane 0 (routed by earlier
        // sibling s) → m's own lane 1 is released and both parent edges go
        // diagonal out of the node.
        let (rows, state) = layout_all(&[
            commit("s", &["x"]),
            commit("m", &["x", "y"]),
            commit("x", &["z"]),
            commit("y", &["z"]),
            commit("z", &[]),
        ]);
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[1].lane, 1, "m takes the next free lane");
        assert_eq!(
            edges_flag(&rows[1], true, false),
            vec![(1, 0), (1, 2)],
            "both parents diagonal from m's node"
        );
        // After the whole stream every flow has ended at the root.
        assert_eq!(state.active_count(), 0);
        assert_eq!(state.width, 3);
    }

    #[test]
    fn freed_lane_is_reused_for_next_branch() {
        let (rows, state) = layout_all(&[
            commit("c", &["b"]),
            commit("d", &["b"]),
            commit("b", &["a"]),
            commit("e", &["a"]),
            commit("a", &[]),
        ]);
        assert_eq!(rows[1].lane, 1, "first fork takes lane 1");
        assert_eq!(rows[3].lane, 1, "second fork reuses freed lane 1");
        assert_eq!(state.width, 2);
        assert_eq!(edges_flag(&rows[3], true, false), vec![(1, 0)]);
    }

    /// Core rendering invariant: every edge crossing a row boundary must
    /// continue on the next row at the same lane — otherwise virtualized
    /// rows would draw disconnected lines.
    #[test]
    fn every_row_connects_to_previous_via_shared_lanes() {
        let (rows, _) = layout_all(&[
            commit("m", &["x", "y"]),
            commit("x", &["z"]),
            commit("y", &["z"]),
            commit("z", &[]),
        ]);
        for w in rows.windows(2) {
            let (lower, upper) = (&w[0], &w[1]);
            for e in &lower.edges {
                // Where does this edge arrive at the bottom of `lower`?
                // to_node-only edges terminate at the node — skip them.
                let arrive = if e.from_node {
                    Some(e.to)
                } else if e.to_node {
                    None
                } else {
                    Some(e.from)
                };
                let Some(arrive) = arrive else { continue };
                let found = upper.edges.iter().any(|u| match (u.from_node, u.to_node) {
                    // Inflow into upper's node must land on its lane.
                    (_, true) => u.from == arrive && upper.lane == arrive,
                    // Pass-through or a lower-half edge starting from the
                    // top boundary... lower-half edges start at the node,
                    // so only pass-through receives from above.
                    (false, false) => u.from == arrive,
                    (true, false) => false,
                });
                assert!(
                    found,
                    "edge arriving lane {arrive} on row {} must continue on row {}",
                    lower.commit.hash, upper.commit.hash
                );
            }
        }
    }

    #[test]
    fn multiple_roots_share_columns_via_free_list() {
        let (rows, state) = layout_all(&[
            commit("b", &["a"]),
            commit("a", &[]),
            commit("d", &["c"]),
            commit("c", &[]),
        ]);
        // The first chain's lane is released before the second root, so
        // both chains share column 0 (free-list reuse keeps width minimal).
        assert_eq!(rows[0].lane, 0);
        assert_eq!(rows[2].lane, 0);
        assert_eq!(state.width, 1);
        assert_eq!(state.active_count(), 0, "both roots end as leaves");
    }

    #[test]
    fn resumable_layout_matches_single_pass() {
        // Laying out in two batches (500/批 增量) must equal one pass.
        let commits = vec![
            commit("m", &["x", "y"]),
            commit("x", &["z"]),
            commit("y", &["z"]),
            commit("z", &[]),
        ];
        let (one_pass, _) = layout_all(&commits);
        let mut state = LayoutState::new();
        let mut rows = Vec::new();
        state.layout(&commits[..2], &mut rows);
        state.layout(&commits[2..], &mut rows);
        assert_eq!(rows, one_pass);
    }

    /// Generate a topo-valid fixture: oldest→newest (parents already
    /// exist), then reversed into topo order. Main line with a merge every
    /// 8th main commit; the merged branch chain is extended 2 commits past
    /// each merge point so future merges have fresh tips.
    fn generate_fixture(n: usize) -> Vec<CommitInfo> {
        let mut oldest_first: Vec<CommitInfo> = Vec::with_capacity(n);
        let mut branch_tips: Vec<Option<usize>> = vec![None; 3];
        let mut main_prev: Option<usize> = None;
        let mut next_id = 0usize;
        let mut main_count = 0usize;
        while oldest_first.len() < n {
            let cur = next_id;
            next_id += 1;
            let mut parents: Vec<String> = Vec::new();
            if let Some(p) = main_prev {
                parents.push(format!("c{p}"));
            }
            let is_merge = main_prev.is_some() && main_count.is_multiple_of(8);
            if is_merge {
                let b = (main_count / 8) % 3;
                if let Some(t) = branch_tips[b] {
                    parents.push(format!("c{t}"));
                }
            }
            // The merge commit itself must be pushed BEFORE its extension
            // chain (the chain hangs off it, i.e. is newer).
            oldest_first.push(commit(
                &format!("c{cur}"),
                &parents.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            ));
            if is_merge && oldest_first.len() + 2 < n {
                let b = (main_count / 8) % 3;
                let mut prev = cur;
                for _ in 0..2 {
                    let next = next_id;
                    next_id += 1;
                    oldest_first.push(commit(&format!("c{next}"), &[&format!("c{prev}")]));
                    prev = next;
                }
                branch_tips[b] = Some(prev);
            }
            main_prev = Some(cur);
            main_count += 1;
        }
        oldest_first.into_iter().rev().collect()
    }

    /// PLAN P5 布局性能目标：10 万提交拓扑（约 1/11 合并密度、3 条长命分支）
    /// 布局进入几十毫秒级。#[ignore]：按需 `cargo test -- --ignored` 在
    /// 基准机实测（CI 不跑）。
    #[test]
    #[ignore]
    fn layout_100k_commits_within_tens_of_ms() {
        use std::time::Instant;
        let commits = generate_fixture(100_000);

        let mut state = LayoutState::new();
        let mut rows = Vec::with_capacity(commits.len());
        let t0 = Instant::now();
        state.layout(&commits, &mut rows);
        let elapsed = t0.elapsed();
        println!(
            "layout {} commits: {elapsed:?}, width={}",
            commits.len(),
            state.width
        );
        assert!(elapsed.as_millis() < 500, "layout took {elapsed:?}");
    }
}
