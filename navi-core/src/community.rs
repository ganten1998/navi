//! Community detection: Louvain modularity optimisation (with a resolution
//! parameter) plus Leiden's guarantee that every community is connected.
//!
//! Full Leiden refines each community with a randomised merge phase before
//! aggregating. Here the refinement is the part that matters for a note
//! graph: after every local-moving phase each community is split into its
//! connected pieces (Louvain can leave a community held together only by a
//! node that has since moved away), and the pieces are what get aggregated.
//! By induction every final community is connected in the input graph.
//!
//! Deterministic: nodes are visited in an order shuffled by a fixed-seed LCG,
//! ties keep a node where it is (then the first candidate in adjacency
//! order), and communities are numbered by size (largest first), then by
//! their smallest node index. Same graph, same seed: same partition.

use std::collections::BTreeMap;

pub const DEFAULT_SEED: u64 = 0x6e61_7669; // "navi"

struct Level {
    adj:    Vec<Vec<(usize, f64)>>, // symmetric, no self entries
    self_w: Vec<f64>,               // self-loop weight (an aggregated community's inner edges)
}

impl Level {
    fn strength(&self, i: usize) -> f64 { self.adj[i].iter().map(|e| e.1).sum::<f64>() + 2.0 * self.self_w[i] }
}

fn lcg(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    *state >> 33
}

fn shuffled(n: usize, seed: &mut u64) -> Vec<usize> {
    let mut v: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() { let j = (lcg(seed) % (i as u64 + 1)) as usize; v.swap(i, j); }
    v
}

/// Louvain local moving: each node joins the neighbouring community with the
/// best modularity gain `k_i,in(c) − γ·Σtot(c)·k_i / 2m`, until a full pass
/// moves nothing (or 64 passes).
fn local_moving(l: &Level, k: &[f64], m2: f64, gamma: f64, order: &[usize]) -> Vec<usize> {
    let n = l.adj.len();
    let mut comm: Vec<usize> = (0..n).collect();
    let mut tot:  Vec<f64>   = k.to_vec();
    let mut w_to = vec![0.0f64; n];
    let mut touched: Vec<usize> = Vec::new();
    for _ in 0..64 {
        let mut moved = false;
        for &i in order {
            let ci = comm[i];
            for &(j, w) in &l.adj[i] {
                let c = comm[j];
                if w_to[c] == 0.0 { touched.push(c); }
                w_to[c] += w;
            }
            tot[ci] -= k[i];
            let gain = |c: usize| w_to[c] - gamma * tot[c] * k[i] / m2;
            let (mut best, mut best_gain) = (ci, gain(ci));
            for &c in &touched {
                let gn = gain(c);
                if gn > best_gain + 1e-12 { best = c; best_gain = gn; }
            }
            tot[best] += k[i];
            if best != ci { comm[i] = best; moved = true; }
            for &c in &touched { w_to[c] = 0.0; }
            touched.clear();
        }
        if !moved { break; }
    }
    comm
}

/// Split every community into its connected pieces and renumber 0..k in
/// order of first node. Returns (community per node, k).
fn split_connected(l: &Level, comm: &[usize]) -> (Vec<usize>, usize) {
    let n = comm.len();
    let mut out = vec![usize::MAX; n];
    let mut k = 0;
    let mut stack = Vec::new();
    for s in 0..n {
        if out[s] != usize::MAX { continue; }
        out[s] = k;
        stack.push(s);
        while let Some(cur) = stack.pop() {
            for &(nb, w) in &l.adj[cur] {
                if w > 0.0 && out[nb] == usize::MAX && comm[nb] == comm[s] { out[nb] = k; stack.push(nb); }
            }
        }
        k += 1;
    }
    (out, k)
}

fn aggregate(l: &Level, comm: &[usize], k: usize) -> Level {
    let mut self_w = vec![0.0f64; k];
    let mut between: BTreeMap<(usize, usize), f64> = BTreeMap::new();
    for i in 0..l.adj.len() {
        let ci = comm[i];
        self_w[ci] += l.self_w[i];
        for &(j, w) in &l.adj[i] {
            if j < i { continue; } // each undirected edge once
            let cj = comm[j];
            if ci == cj { self_w[ci] += w; } else { *between.entry((ci.min(cj), ci.max(cj))).or_insert(0.0) += w; }
        }
    }
    let mut adj = vec![Vec::new(); k];
    for ((a, b), w) in between { adj[a].push((b, w)); adj[b].push((a, w)); }
    Level { adj, self_w }
}

/// Community of every node of an undirected, unweighted graph (`edges` over
/// 0..n; duplicates and self-links are tolerated). `gamma` is the resolution:
/// above 1 gives more, smaller communities; below 1 fewer, larger ones.
/// Isolated nodes are communities of their own.
pub fn communities(n: usize, edges: &[(usize, usize)], gamma: f64, seed: u64) -> Vec<usize> {
    if n == 0 { return Vec::new(); }
    let gamma = if gamma.is_finite() && gamma > 0.0 { gamma } else { 1.0 };
    let mut pairs: BTreeMap<(usize, usize), f64> = BTreeMap::new();
    for &(a, b) in edges {
        if a == b || a >= n || b >= n { continue; }
        pairs.insert((a.min(b), a.max(b)), 1.0);
    }
    let mut adj = vec![Vec::new(); n];
    for (&(a, b), &w) in &pairs { adj[a].push((b, w)); adj[b].push((a, w)); }
    let mut level = Level { adj, self_w: vec![0.0; n] };
    let mut member: Vec<usize> = (0..n).collect(); // original node → node of the current level
    let mut seed = seed ^ 0x9e37_79b9_7f4a_7c15;

    for _ in 0..32 {
        let ln = level.adj.len();
        let k: Vec<f64> = (0..ln).map(|i| level.strength(i)).collect();
        let m2: f64 = k.iter().sum();
        if m2 <= 0.0 { break; }
        let order = shuffled(ln, &mut seed);
        let comm  = local_moving(&level, &k, m2, gamma, &order);
        let (comm, kc) = split_connected(&level, &comm);
        if kc == ln { break; } // nothing merged: converged
        for m in member.iter_mut() { *m = comm[*m]; }
        level = aggregate(&level, &comm, kc);
    }

    // number by size desc, then smallest node index
    let k = member.iter().copied().max().map_or(0, |m| m + 1);
    let mut size  = vec![0usize; k];
    let mut first = vec![usize::MAX; k];
    for (i, &c) in member.iter().enumerate() { size[c] += 1; first[c] = first[c].min(i); }
    let mut order: Vec<usize> = (0..k).filter(|&c| size[c] > 0).collect();
    order.sort_by_key(|&c| (std::cmp::Reverse(size[c]), first[c]));
    let mut rank = vec![0usize; k];
    for (r, &c) in order.iter().enumerate() { rank[c] = r; }
    member.iter().map(|&c| rank[c]).collect()
}

/// Newman modularity of a partition at resolution `gamma` (for tests and
/// diagnostics).
pub fn modularity(n: usize, edges: &[(usize, usize)], comm: &[usize], gamma: f64) -> f64 {
    let mut pairs: BTreeMap<(usize, usize), ()> = BTreeMap::new();
    for &(a, b) in edges { if a != b && a < n && b < n { pairs.insert((a.min(b), a.max(b)), ()); } }
    let m = pairs.len() as f64;
    if m == 0.0 { return 0.0; }
    let mut deg = vec![0.0f64; n];
    let mut inner: BTreeMap<usize, f64> = BTreeMap::new();
    for &(a, b) in pairs.keys() {
        deg[a] += 1.0; deg[b] += 1.0;
        if comm[a] == comm[b] { *inner.entry(comm[a]).or_insert(0.0) += 1.0; }
    }
    let mut tot: BTreeMap<usize, f64> = BTreeMap::new();
    for i in 0..n { *tot.entry(comm[i]).or_insert(0.0) += deg[i]; }
    tot.iter().map(|(c, &t)| inner.get(c).copied().unwrap_or(0.0) / m - gamma * (t / (2.0 * m)).powi(2)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `k` cliques of size `s`, clique i joined to clique i+1 by one edge (a ring).
    fn ring_of_cliques(k: usize, s: usize) -> (usize, Vec<(usize, usize)>) {
        let mut e = Vec::new();
        for c in 0..k {
            for a in 0..s { for b in a + 1..s { e.push((c * s + a, c * s + b)); } }
            e.push((c * s, ((c + 1) % k) * s + 1));
        }
        (k * s, e)
    }

    #[test]
    fn finds_the_cliques_of_a_ring() {
        let (n, e) = ring_of_cliques(8, 5);
        let c = communities(n, &e, 1.0, DEFAULT_SEED);
        for q in 0..8 {
            let first = c[q * 5];
            assert!((0..5).all(|a| c[q * 5 + a] == first), "clique {q} split: {c:?}");
        }
        let mut distinct: Vec<usize> = c.clone(); distinct.sort(); distinct.dedup();
        assert_eq!(distinct.len(), 8, "{c:?}");
        assert!(modularity(n, &e, &c, 1.0) > 0.7);
    }

    #[test]
    fn deterministic_and_numbered_by_size() {
        // a big clique, a small clique, a pair and two isolated nodes
        let mut e = Vec::new();
        for a in 0..6 { for b in a + 1..6 { e.push((a, b)); } }
        for a in 6..9 { for b in a + 1..9 { e.push((a, b)); } }
        e.push((9, 10));
        e.push((5, 6)); e.push((5, 6)); e.push((3, 3)); // duplicate + self link tolerated
        let c1 = communities(13, &e, 1.0, DEFAULT_SEED);
        let c2 = communities(13, &e, 1.0, DEFAULT_SEED);
        assert_eq!(c1, c2);
        assert_eq!(&c1[..6], &[0; 6]);
        assert_eq!(&c1[6..9], &[1; 3]);
        assert_eq!(c1[9], c1[10]);
        assert_ne!(c1[11], c1[12], "isolated nodes are singletons");
        // any seed: the same partition on this clear-cut graph (labels by size)
        assert_eq!(communities(13, &e, 1.0, 12345), c1);
    }

    #[test]
    fn resolution_changes_granularity() {
        let (n, e) = ring_of_cliques(12, 4);
        let count = |g: f64| { let mut c = communities(n, &e, g, DEFAULT_SEED); c.sort(); c.dedup(); c.len() };
        assert!(count(0.05) < count(1.0), "low resolution merges cliques");
        assert!(count(4.0) >= count(1.0));
        assert_eq!(communities(0, &[], 1.0, DEFAULT_SEED), Vec::<usize>::new());
        assert_eq!(communities(3, &[], 1.0, DEFAULT_SEED).len(), 3);
        assert_eq!(communities(2, &[(0, 1)], f64::NAN, DEFAULT_SEED), vec![0, 0], "bad gamma falls back to 1");
    }

    /// Every community is connected in the input graph (the Leiden guarantee).
    #[test]
    fn communities_are_connected() {
        let mut seed = 7u64;
        for trial in 0..20 {
            let n = 60;
            let mut e = Vec::new();
            for _ in 0..90 { let a = (lcg(&mut seed) % n) as usize; let b = (lcg(&mut seed) % n) as usize; e.push((a, b)); }
            let c = communities(n as usize, &e, 1.0, trial);
            let mut adj = vec![Vec::new(); n as usize];
            for &(a, b) in &e { if a != b { adj[a].push(b); adj[b].push(a); } }
            for comm in 0..n as usize {
                let mem: Vec<usize> = (0..n as usize).filter(|&i| c[i] == comm).collect();
                if mem.len() < 2 { continue; }
                let mut seen = vec![false; n as usize];
                let mut stack = vec![mem[0]]; seen[mem[0]] = true;
                while let Some(x) = stack.pop() {
                    for &y in &adj[x] { if !seen[y] && c[y] == comm { seen[y] = true; stack.push(y); } }
                }
                assert!(mem.iter().all(|&i| seen[i]), "trial {trial}: community {comm} disconnected");
            }
        }
    }

    /// 10k nodes in well under a second even in a debug build; release is
    /// what the app runs (and it runs off the UI thread above a threshold).
    #[test]
    fn ten_thousand_nodes_is_fast() {
        let mut seed = 99u64;
        let n = 10_000usize;
        let mut e = Vec::new();
        // clustered: 100 clusters of 100, ~2.5 edges/node, 10% cross-cluster
        for i in 0..n {
            for _ in 0..3 {
                let r = lcg(&mut seed);
                let j = if r % 10 == 7 { (lcg(&mut seed) as usize) % n } else { (i / 100) * 100 + (lcg(&mut seed) as usize) % 100 };
                e.push((i, j));
            }
        }
        let t = std::time::Instant::now();
        let c = communities(n, &e, 1.0, DEFAULT_SEED);
        let dt = t.elapsed();
        assert_eq!(c.len(), n);
        let q = modularity(n, &e, &c, 1.0);
        assert!(q > 0.6, "modularity {q}");
        assert!(dt.as_secs_f64() < 10.0, "{dt:?}");
        eprintln!("communities: 10k nodes {} edges in {dt:?}, Q = {q:.3}", e.len());
    }
}
