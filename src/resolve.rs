use std::collections::{HashMap, HashSet};

use crate::graph::{CKind, ContractInfo, Edge, FKind, Graph, Node, Vis};
use crate::index::{Index, RawCall};

/// A contract whose functions are expanded into graph nodes.
struct Ctx {
    /// Contract index.
    k: usize,
    lin_set: HashSet<usize>,
    /// Maps every function in an override chain to the effective (most derived) function.
    eff: HashMap<usize, usize>,
    /// Implemented functions declared anywhere in the linearization.
    members: Vec<usize>,
}

struct Builder<'a> {
    ix: &'a Index,
    ctxs: Vec<Ctx>,
    node_of: HashMap<(usize, usize), u32>,
    /// contract index -> ctx indices of concrete contracts inheriting it.
    implementers: HashMap<usize, Vec<usize>>,
    sel_map: HashMap<[u8; 4], Vec<(usize, u32)>>,
    ctor_root: HashMap<usize, u32>,
    ext_cache: HashMap<usize, Vec<Edge>>,
}

fn is_context(ix: &Index, c: usize) -> bool {
    let contract = &ix.contracts[c];
    match contract.kind {
        CKind::Contract => !contract.is_abstract,
        CKind::Library | CKind::Free => true,
        CKind::Interface => false,
    }
}

fn related(ix: &Index, g: usize, f: usize) -> bool {
    g == f || ix.bases_closure(g).contains(&f) || ix.bases_closure(f).contains(&g)
}

fn make_ctx(ix: &Index, k: usize) -> Ctx {
    let lin = &ix.contracts[k].lin;
    let mut eff: HashMap<usize, usize> = HashMap::new();
    let mut members = Vec::new();
    for &c in lin {
        for &g in &ix.contracts[c].funcs {
            let func = &ix.funcs[g];
            if func.implemented {
                members.push(g);
            }
            if func.kind == FKind::Constructor {
                continue;
            }
            let target = eff.get(&g).copied().unwrap_or(g);
            eff.insert(g, target);
            for b in &func.base {
                if let Some(&bi) = ix.func_by_id.get(b) {
                    eff.entry(bi).or_insert(target);
                }
            }
        }
    }
    Ctx {
        k,
        lin_set: lin.iter().copied().collect(),
        eff,
        members,
    }
}

pub fn build_graph(ix: &Index, mut diag: Vec<String>) -> Graph {
    let mut dup_names: HashMap<&str, usize> = HashMap::new();
    for c in &ix.contracts {
        *dup_names.entry(&c.name).or_default() += 1;
    }
    let contracts: Vec<ContractInfo> = ix
        .contracts
        .iter()
        .map(|c| ContractInfo {
            name: c.name.clone(),
            display: if dup_names[c.name.as_str()] > 1 {
                format!("{}:{}", c.path, c.name)
            } else {
                c.name.clone()
            },
            path: c.path.clone(),
            kind: c.kind,
        })
        .collect();

    let ctxs: Vec<Ctx> = (0..ix.contracts.len())
        .filter(|&c| is_context(ix, c))
        .map(|c| make_ctx(ix, c))
        .collect();

    let mut b = Builder {
        ix,
        ctxs,
        node_of: HashMap::new(),
        implementers: HashMap::new(),
        sel_map: HashMap::new(),
        ctor_root: HashMap::new(),
        ext_cache: HashMap::new(),
    };

    let mut nodes: Vec<Node> = Vec::new();
    let mut ctor_chains: Vec<(u32, Vec<u32>)> = Vec::new();

    for ci in 0..b.ctxs.len() {
        let k = b.ctxs[ci].k;
        let kind = ix.contracts[k].kind;
        let effective = |ctx: &Ctx, f: usize| ctx.eff.get(&f).copied().unwrap_or(f) == f;
        let mut name_count: HashMap<&str, usize> = HashMap::new();
        for &f in &b.ctxs[ci].members {
            let func = &ix.funcs[f];
            if func.kind == FKind::Function && effective(&b.ctxs[ci], f) {
                *name_count.entry(&func.name).or_default() += 1;
            }
        }
        let mut ctors = Vec::new();
        for &f in &b.ctxs[ci].members.clone() {
            let func = &ix.funcs[f];
            let eff = effective(&b.ctxs[ci], f);
            let entry = eff
                && matches!(kind, CKind::Contract | CKind::Library)
                && matches!(
                    func.kind,
                    FKind::Function | FKind::Receive | FKind::Fallback | FKind::Getter
                )
                && matches!(func.vis, Vis::External | Vis::Public);
            let n = nodes.len() as u32;
            nodes.push(Node {
                ctx: k as u32,
                decl: func.contract as u32,
                name: func.name.clone(),
                sig: func.sig.clone(),
                selector: func.selector,
                kind: func.kind,
                vis: func.vis,
                entry,
                overloaded: func.kind == FKind::Function
                    && name_count.get(func.name.as_str()).copied().unwrap_or(0) > 1,
            });
            b.node_of.insert((k, f), n);
            if func.kind == FKind::Constructor && kind == CKind::Contract {
                ctors.push(n);
            }
            if entry
                && kind == CKind::Contract
                && let Some(sel) = func.selector
            {
                b.sel_map.entry(sel).or_default().push((ci, n));
            }
        }
        if let Some((&root, rest)) = ctors.split_first() {
            nodes[root as usize].entry = true;
            b.ctor_root.insert(k, root);
            ctor_chains.push((root, rest.to_vec()));
        }
        if kind == CKind::Contract {
            for &c in &ix.contracts[k].lin {
                b.implementers.entry(c).or_default().push(ci);
            }
        }
    }

    let mut adj: Vec<Vec<Edge>> = vec![Vec::new(); nodes.len()];
    for ci in 0..b.ctxs.len() {
        let k = b.ctxs[ci].k;
        for f in b.ctxs[ci].members.clone() {
            let from = b.node_of[&(k, f)];
            let mut edges = Vec::new();
            for call in &ix.funcs[f].calls {
                edges.extend(b.resolve(ci, f, *call));
            }
            adj[from as usize] = edges;
        }
    }
    for (root, rest) in ctor_chains {
        adj[root as usize].extend(rest.into_iter().map(|to| Edge { to, approx: false }));
    }
    for edges in &mut adj {
        edges.sort_by_key(|e| (e.to, e.approx));
        edges.dedup_by_key(|e| e.to);
    }

    if ix.collisions > 0 {
        diag.push(format!(
            "warning: {} node-id collisions across sources (multiple compilation jobs?); call resolution may be wrong",
            ix.collisions
        ));
    }
    let mismatched: u32 = ix.funcs.iter().map(|f| f.mismatched).sum();
    if mismatched > 0 {
        diag.push(format!(
            "warning: {mismatched} call sites reference unknown declarations or a function with a \
             different name; artifacts likely come from different compilation jobs (partial \
             build), or imports are aliased; results may be wrong"
        ));
    }

    Graph {
        contracts,
        nodes,
        adj,
        diag,
    }
}

impl Builder<'_> {
    fn node(&self, k: usize, f: usize) -> Option<u32> {
        self.node_of.get(&(k, f)).copied()
    }

    fn direct(&self, k: usize, f: usize) -> Vec<Edge> {
        self.node(k, f)
            .map(|to| vec![Edge { to, approx: false }])
            .unwrap_or_default()
    }

    fn resolve(&mut self, ci: usize, from: usize, call: RawCall) -> Vec<Edge> {
        let ix = self.ix;
        let k = self.ctxs[ci].k;
        match call {
            RawCall::Internal(f) | RawCall::Modifier(f) => {
                let owner = ix.funcs[f].contract;
                match ix.contracts[owner].kind {
                    CKind::Library | CKind::Free => self.direct(owner, f),
                    _ => {
                        let e = self.ctxs[ci].eff.get(&f).copied().unwrap_or(f);
                        self.direct(k, e)
                    }
                }
            }
            RawCall::Specific(f) => {
                let owner = ix.funcs[f].contract;
                match ix.contracts[owner].kind {
                    CKind::Library | CKind::Free => self.direct(owner, f),
                    _ if self.ctxs[ci].lin_set.contains(&owner) => self.direct(k, f),
                    _ => Vec::new(),
                }
            }
            RawCall::Super(f) => {
                let declaring = ix.funcs[from].contract;
                let lin = &ix.contracts[k].lin;
                let Some(pos) = lin.iter().position(|&c| c == declaring) else {
                    return Vec::new();
                };
                for &c in &lin[pos + 1..] {
                    for &g in &ix.contracts[c].funcs {
                        let cand = &ix.funcs[g];
                        if cand.implemented
                            && cand.kind != FKind::Constructor
                            && cand.name == ix.funcs[f].name
                            && related(ix, g, f)
                        {
                            return self.direct(k, g);
                        }
                    }
                }
                Vec::new()
            }
            RawCall::This(f) => match self.ctxs[ci].eff.get(&f).copied() {
                Some(e) => self.direct(k, e),
                None => self.external(f),
            },
            RawCall::External(f) => self.external(f),
            RawCall::New(c) => self
                .ctor_root
                .get(&c)
                .map(|&to| vec![Edge { to, approx: false }])
                .unwrap_or_default(),
        }
    }

    fn external(&mut self, f: usize) -> Vec<Edge> {
        if let Some(cached) = self.ext_cache.get(&f) {
            return cached.clone();
        }
        let ix = self.ix;
        let owner = ix.funcs[f].contract;
        let mut out = Vec::new();
        match ix.contracts[owner].kind {
            CKind::Library | CKind::Free => out.extend(self.direct(owner, f)),
            _ => {
                let owner_concrete = is_context(ix, owner);
                for &ci2 in self
                    .implementers
                    .get(&owner)
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                {
                    let ctx2 = &self.ctxs[ci2];
                    if let Some(&e) = ctx2.eff.get(&f)
                        && let Some(to) = self.node(ctx2.k, e)
                    {
                        out.push(Edge {
                            to,
                            approx: !(owner_concrete && ctx2.k == owner),
                        });
                    }
                }
                if let Some(sel) = ix.funcs[f].selector {
                    for &(ci2, to) in self.sel_map.get(&sel).map(Vec::as_slice).unwrap_or(&[]) {
                        if !self.ctxs[ci2].lin_set.contains(&owner) {
                            out.push(Edge { to, approx: true });
                        }
                    }
                }
            }
        }
        self.ext_cache.insert(f, out.clone());
        out
    }
}
