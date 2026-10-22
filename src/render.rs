use std::collections::HashMap;
use std::io::{self, Write};

use crate::graph::{Edge, Graph};

/// Which call chains to show.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Only chains made entirely of certain edges.
    Hard,
    /// Only chains containing at least one `~` (guessed) edge.
    Soft,
}

/// A search state is `(node, seen_approx)` packed as `node * 2 + seen`, where
/// `seen_approx` says whether the chain from the node down to the function of
/// interest contains a guessed edge. Hard mode only ever uses `seen = 0`; soft
/// mode needs the whole chain, entry point included, to have seen one.
fn state(n: u32, seen: bool) -> usize {
    n as usize * 2 + seen as usize
}

fn allowed(mode: Mode, e: &Edge) -> bool {
    mode == Mode::Soft || !e.approx
}

struct Renderer<'a, W: Write> {
    g: &'a Graph,
    out: W,
    /// callers[n] = (caller, the call is a guess)
    callers: Vec<Vec<(u32, bool)>>,
    /// State can be extended upward, through callers, to an entry point.
    up: Vec<bool>,
    labels: Vec<String>,
    names: Vec<String>,
    on_path: Vec<bool>,
}

/// Print the function(s) of interest, then, indented below, everything that
/// calls them, and so on up to the entry points (marked `<-`).
/// Returns false when no entry point reaches a target in this mode.
pub fn render<W: Write>(g: &Graph, targets: &[u32], mode: Mode, out: W) -> io::Result<bool> {
    let n = g.nodes.len();
    let goal_seen = mode == Mode::Soft;

    let mut callers: Vec<Vec<(u32, bool)>> = vec![Vec::new(); n];
    for (from, edges) in g.adj.iter().enumerate() {
        for e in edges.iter().filter(|e| allowed(mode, e)) {
            callers[e.to as usize].push((from as u32, e.approx));
        }
    }

    // Forward from the entry points: (n, s) is extendable upward when some
    // caller state is. A guessed edge only needs the caller side to have seen
    // one; a certain edge passes the flag along unchanged.
    let mut up = vec![false; n * 2];
    let mut stack = Vec::new();
    for (i, node) in g.nodes.iter().enumerate() {
        if node.entry {
            let st = state(i as u32, goal_seen);
            up[st] = true;
            stack.push(st);
        }
    }
    while let Some(st) = stack.pop() {
        let (c, seen) = ((st / 2) as u32, st % 2 == 1);
        for e in g.adj[c as usize].iter().filter(|e| allowed(mode, e)) {
            let marks: &[bool] = match (e.approx, seen) {
                (true, true) => &[false, true],
                (true, false) => &[],
                (false, true) => &[true],
                (false, false) => &[false],
            };
            for &s in marks {
                let next = state(e.to, s);
                if !up[next] {
                    up[next] = true;
                    stack.push(next);
                }
            }
        }
    }

    let labels: Vec<String> = (0..n as u32).map(|i| g.label(i)).collect();
    let names: Vec<String> = (0..n as u32).map(|i| g.name(i)).collect();
    let mut r = Renderer {
        g,
        out,
        callers,
        up,
        labels,
        names,
        on_path: vec![false; n],
    };

    let mut roots: Vec<u32> = targets
        .iter()
        .copied()
        .filter(|&t| r.up[state(t, false)])
        .collect();
    roots.sort_by(|&a, &b| r.labels[a as usize].cmp(&r.labels[b as usize]));
    roots.dedup();
    let roots = r.drop_nested(roots);

    let found = !roots.is_empty();
    for root in roots {
        r.visit(root, false, false, 0)?;
    }
    Ok(found)
}

impl<W: Write> Renderer<'_, W> {
    /// Callers of `(v, seen)` that lead up to an entry point, by label.
    fn kids(&self, v: u32, seen: bool) -> Vec<(u32, bool)> {
        let mut kids: Vec<(u32, bool)> = self.callers[v as usize]
            .iter()
            .copied()
            .filter(|&(c, approx)| self.up[state(c, seen || approx)])
            .collect();
        kids.sort_by(|a, b| self.labels[a.0 as usize].cmp(&self.labels[b.0 as usize]));
        kids
    }

    /// A target that is itself a caller of another target already shows up in
    /// that target's tree. Of targets that call each other, the first by label stays.
    fn drop_nested(&self, roots: Vec<u32>) -> Vec<u32> {
        let index_of: HashMap<u32, usize> = roots.iter().enumerate().map(|(i, &r)| (r, i)).collect();
        let contains: Vec<Vec<bool>> = roots
            .iter()
            .map(|&root| {
                let mut contained = vec![false; roots.len()];
                let mut seen = vec![false; self.up.len()];
                let mut stack = vec![(root, false)];
                seen[state(root, false)] = true;
                while let Some((v, s)) = stack.pop() {
                    for (c, approx) in self.kids(v, s) {
                        let ns = s || approx;
                        if !seen[state(c, ns)] {
                            seen[state(c, ns)] = true;
                            if c != root
                                && let Some(&i) = index_of.get(&c)
                            {
                                contained[i] = true;
                            }
                            stack.push((c, ns));
                        }
                    }
                }
                contained
            })
            .collect();
        (0..roots.len())
            .filter(|&r| {
                !(0..roots.len()).any(|o| o != r && contains[o][r] && !(contains[r][o] && o > r))
            })
            .map(|r| roots[r])
            .collect()
    }

    /// Every line names the declaring contract, where the code lives. Entry
    /// points also give the contract they are called on, after the arrow.
    fn visit(&mut self, v: u32, approx: bool, seen: bool, depth: usize) -> io::Result<()> {
        let mut line = format!(
            "{}{}{}",
            "  ".repeat(depth),
            if approx { "~" } else { "" },
            self.names[v as usize]
        );
        if self.g.nodes[v as usize].entry {
            line.push_str(" <-");
            if let Some(ctx) = self.g.inherited_ctx(v) {
                line.push_str(&format!(" {ctx}"));
            }
        }
        if self.on_path[v as usize] {
            line.push_str(" (recursive)");
            return writeln!(self.out, "{line}");
        }
        writeln!(self.out, "{line}")?;
        self.on_path[v as usize] = true;
        for (c, approx_edge) in self.kids(v, seen) {
            self.visit(c, approx_edge, seen || approx_edge, depth + 1)?;
        }
        self.on_path[v as usize] = false;
        Ok(())
    }
}
