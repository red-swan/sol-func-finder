use std::collections::HashMap;
use std::io::{self, Write};

use crate::graph::{Edge, EdgeKind, Graph};

/// Which call chains to show.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Only chains made entirely of certain edges.
    Hard,
    /// Only chains containing at least one guessed edge, in one section per
    /// kind of guess (the loosest edge of the chain decides the section).
    Soft,
}

impl Mode {
    /// The chain classes this mode prints, one section each.
    fn goals(self) -> &'static [EdgeKind] {
        match self {
            Mode::Hard => &[EdgeKind::Certain],
            Mode::Soft => &EdgeKind::GUESSED,
        }
    }
}

const LEVELS: usize = 4;

/// A search state is `(node, level)` packed as `node * LEVELS + level`, where
/// `level` is the loosest edge kind on the chain from the node down to the
/// function of interest. A search runs for one goal level: it only follows
/// edges up to that kind, and a chain counts only when, entry point included,
/// its loosest edge is exactly the goal.
fn state(n: u32, level: EdgeKind) -> usize {
    n as usize * LEVELS + level as usize
}

struct Renderer<'a, W: Write> {
    g: &'a Graph,
    out: W,
    /// callers[n] = (caller, kind of the call)
    callers: Vec<Vec<(u32, EdgeKind)>>,
    /// State can be extended upward, through callers, to an entry point.
    up: Vec<bool>,
    labels: Vec<String>,
    names: Vec<String>,
    on_path: Vec<bool>,
    goal: EdgeKind,
}

/// Print the function(s) of interest, then, indented below, everything that
/// calls them, and so on up to the entry points (marked `<-`). Soft mode
/// prints one headed section per kind of guess.
/// Returns false when no entry point reaches a target in this mode.
pub fn render<W: Write>(g: &Graph, targets: &[u32], mode: Mode, mut out: W) -> io::Result<bool> {
    let mut any = false;
    for &goal in mode.goals() {
        let mut r = Renderer::new(g, goal, &mut out);
        let mut roots: Vec<u32> = targets
            .iter()
            .copied()
            .filter(|&t| r.up[state(t, EdgeKind::Certain)])
            .collect();
        roots.sort_by(|&a, &b| r.labels[a as usize].cmp(&r.labels[b as usize]));
        roots.dedup();
        let roots = r.drop_nested(roots);
        if roots.is_empty() {
            continue;
        }
        if mode == Mode::Soft {
            if any {
                writeln!(r.out)?;
            }
            writeln!(r.out, "## {}", goal.title())?;
        }
        any = true;

        // One concrete contract per node, but one function: instances of the same
        // declared function (inherited by several contracts) share a top line.
        let mut groups: Vec<Vec<u32>> = Vec::new();
        for root in roots {
            match groups
                .iter_mut()
                .find(|gr| r.names[gr[0] as usize] == r.names[root as usize])
            {
                Some(gr) => gr.push(root),
                None => groups.push(vec![root]),
            }
        }
        for group in groups {
            r.visit_top(&group)?;
        }
    }
    Ok(any)
}

/// The entry points that reach a target in this mode, as `Contract.function`
/// (the contract they are called on), sorted and without repeats; one list per
/// section (a single one in hard mode), empty sections left out.
pub fn entries(
    g: &Graph,
    targets: &[u32],
    mode: Mode,
    keep: impl Fn(u32) -> bool,
) -> Vec<(EdgeKind, Vec<String>)> {
    let mut sections = Vec::new();
    for &goal in mode.goals() {
        let r = Renderer::new(g, goal, io::sink());
        let mut seen = vec![false; r.up.len()];
        let mut stack = Vec::new();
        for &t in targets
            .iter()
            .filter(|&&t| r.up[state(t, EdgeKind::Certain)])
        {
            seen[state(t, EdgeKind::Certain)] = true;
            stack.push((t, EdgeKind::Certain));
        }
        let mut found = Vec::new();
        while let Some((v, level)) = stack.pop() {
            // The chain from the entry point down must have the goal as its loosest edge.
            if g.nodes[v as usize].entry && level == goal && keep(v) {
                found.push(g.concrete_name(v));
            }
            for (c, kind) in r.kids(v, level) {
                let next = level.max(kind);
                if !seen[state(c, next)] {
                    seen[state(c, next)] = true;
                    stack.push((c, next));
                }
            }
        }
        found.sort();
        found.dedup();
        if !found.is_empty() {
            sections.push((goal, found));
        }
    }
    sections
}

impl<'a, W: Write> Renderer<'a, W> {
    fn new(g: &'a Graph, goal: EdgeKind, out: W) -> Self {
        let n = g.nodes.len();
        let allowed = |e: &Edge| e.kind <= goal;

        let mut callers: Vec<Vec<(u32, EdgeKind)>> = vec![Vec::new(); n];
        for (from, edges) in g.adj.iter().enumerate() {
            for e in edges.iter().filter(|e| allowed(e)) {
                callers[e.to as usize].push((from as u32, e.kind));
            }
        }

        // Forward from the entry points: (n, level) is extendable upward when some
        // caller state is. The caller's level is the looser of the edge and the
        // callee's: a looser edge sets it (the callee may be anything up to it),
        // a tighter one passes the callee's level along unchanged.
        let mut up = vec![false; n * LEVELS];
        let mut stack = Vec::new();
        for (i, node) in g.nodes.iter().enumerate() {
            if node.entry {
                let st = state(i as u32, goal);
                up[st] = true;
                stack.push(st);
            }
        }
        while let Some(st) = stack.pop() {
            let (c, level) = ((st / LEVELS) as u32, st % LEVELS);
            for e in g.adj[c as usize].iter().filter(|e| allowed(e)) {
                let k = e.kind as usize;
                let callee_levels = match k.cmp(&level) {
                    std::cmp::Ordering::Less => level..=level,
                    std::cmp::Ordering::Equal => 0..=level,
                    std::cmp::Ordering::Greater => continue,
                };
                for l in callee_levels {
                    let next = e.to as usize * LEVELS + l;
                    if !up[next] {
                        up[next] = true;
                        stack.push(next);
                    }
                }
            }
        }

        let labels: Vec<String> = (0..n as u32).map(|i| g.label(i)).collect();
        let names: Vec<String> = (0..n as u32).map(|i| g.name(i)).collect();
        Renderer {
            g,
            out,
            callers,
            up,
            labels,
            names,
            on_path: vec![false; n],
            goal,
        }
    }
}

impl<W: Write> Renderer<'_, W> {
    /// Callers of `(v, level)` that lead up to an entry point, by label.
    fn kids(&self, v: u32, level: EdgeKind) -> Vec<(u32, EdgeKind)> {
        let mut kids: Vec<(u32, EdgeKind)> = self.callers[v as usize]
            .iter()
            .copied()
            .filter(|&(c, kind)| self.up[state(c, level.max(kind))])
            .collect();
        kids.sort_by(|a, b| self.labels[a.0 as usize].cmp(&self.labels[b.0 as usize]));
        kids
    }

    /// A target that is itself a caller of another target already shows up in
    /// that target's tree. Of targets that call each other, the first by label stays.
    fn drop_nested(&self, roots: Vec<u32>) -> Vec<u32> {
        let index_of: HashMap<u32, usize> =
            roots.iter().enumerate().map(|(i, &r)| (r, i)).collect();
        let contains: Vec<Vec<bool>> = roots
            .iter()
            .map(|&root| {
                let mut contained = vec![false; roots.len()];
                let mut seen = vec![false; self.up.len()];
                let mut stack = vec![(root, EdgeKind::Certain)];
                seen[state(root, EdgeKind::Certain)] = true;
                while let Some((v, level)) = stack.pop() {
                    for (c, kind) in self.kids(v, level) {
                        let next = level.max(kind);
                        if !seen[state(c, next)] {
                            seen[state(c, next)] = true;
                            if c != root
                                && let Some(&i) = index_of.get(&c)
                            {
                                contained[i] = true;
                            }
                            stack.push((c, next));
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

    /// Top line for the instances of one declared function, with the callers of
    /// all instances (each caller once) on the lines below.
    fn visit_top(&mut self, group: &[u32]) -> io::Result<()> {
        let mut line = self.names[group[0] as usize].clone();
        let mut ctxs: Vec<&str> = Vec::new();
        let mut entry = false;
        for &v in group {
            if self.g.nodes[v as usize].entry {
                entry = true;
                if let Some(ctx) = self.g.inherited_ctx(v) {
                    ctxs.push(ctx);
                }
            }
        }
        if entry {
            line.push_str(" <-");
            if !ctxs.is_empty() {
                ctxs.sort();
                ctxs.dedup();
                line.push_str(&format!(" {}", ctxs.join(", ")));
            }
        }
        writeln!(self.out, "{line}")?;
        let mut kids: Vec<(u32, EdgeKind)> = group
            .iter()
            .flat_map(|&v| self.kids(v, EdgeKind::Certain))
            .collect();
        kids.sort_by(|a, b| self.labels[a.0 as usize].cmp(&self.labels[b.0 as usize]));
        kids.dedup();
        for &v in group {
            self.on_path[v as usize] = true;
        }
        for (c, kind) in kids {
            self.visit(c, kind, kind, 1)?;
        }
        for &v in group {
            self.on_path[v as usize] = false;
        }
        Ok(())
    }

    /// Every line names the declaring contract, where the code lives. Entry
    /// points also give the contract they are called on, after the arrow.
    /// `kind` is how the line's function calls the line above; `level` is the
    /// loosest edge from here down to the function of interest.
    fn visit(&mut self, v: u32, kind: EdgeKind, level: EdgeKind, depth: usize) -> io::Result<()> {
        debug_assert!(level <= self.goal);
        let mut line = format!(
            "{}{}{}",
            "  ".repeat(depth),
            kind.marker(),
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
        for (c, edge) in self.kids(v, level) {
            self.visit(c, edge, level.max(edge), depth + 1)?;
        }
        self.on_path[v as usize] = false;
        Ok(())
    }
}
