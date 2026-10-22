use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::ast::{arr, flag, id, nt, refd, s};
use crate::graph::{CKind, FKind, Vis};
use crate::loader::Source;

/// A call site found in a function body, resolved to declared functions but
/// not yet to a concrete contract.
#[derive(Clone, Copy, Debug)]
pub enum RawCall {
    /// `f()`: virtual dispatch in the concrete contract (or direct for library / free functions).
    Internal(usize),
    /// `X.f()` where X is a contract or library name: no virtual dispatch.
    Specific(usize),
    Super(usize),
    This(usize),
    /// `expr.f()`: external call; receiver type is unknown.
    External(usize),
    /// `new X()`, holds the contract index.
    New(usize),
    Modifier(usize),
}

pub struct Contract {
    pub name: String,
    pub path: String,
    pub kind: CKind,
    pub is_abstract: bool,
    pub lin_ids: Vec<i64>,
    /// Linearization (most derived first) restricted to in-scope contracts.
    pub lin: Vec<usize>,
    pub funcs: Vec<usize>,
}

pub struct Func {
    pub id: i64,
    pub name: String,
    pub sig: String,
    pub selector: Option<[u8; 4]>,
    pub kind: FKind,
    pub vis: Vis,
    pub contract: usize,
    pub base: Vec<i64>,
    pub implemented: bool,
    pub calls: Vec<RawCall>,
    /// Call sites whose resolved declaration has a different name than the call.
    pub mismatched: u32,
}

#[derive(Default)]
pub struct Index {
    pub contracts: Vec<Contract>,
    pub contract_by_id: HashMap<i64, usize>,
    pub funcs: Vec<Func>,
    pub func_by_id: HashMap<i64, usize>,
    /// Ids of every declaration (functions, variables, types, events, ...) in scope.
    pub decl_ids: HashSet<i64>,
    pub collisions: usize,
}

impl Index {
    /// Transitive closure of `f`'s overridden functions.
    pub fn bases_closure(&self, f: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut stack = vec![f];
        while let Some(cur) = stack.pop() {
            for b in &self.funcs[cur].base {
                if let Some(&bi) = self.func_by_id.get(b)
                    && !out.contains(&bi)
                {
                    out.push(bi);
                    stack.push(bi);
                }
            }
        }
        out
    }
}

fn parse_selector(v: &Value) -> Option<[u8; 4]> {
    let hex = s(v, "functionSelector");
    if hex.len() != 8 {
        return None;
    }
    let mut out = [0u8; 4];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn norm_type(t: &str) -> String {
    let mut out = t.to_string();
    for w in [" memory", " calldata", " storage ref", " storage pointer", " payable"] {
        out = out.replace(w, "");
    }
    for p in ["contract ", "struct ", "enum "] {
        if let Some(rest) = out.strip_prefix(p) {
            out = rest.to_string();
        }
    }
    out
}

fn signature(name: &str, v: &Value) -> String {
    let params = v
        .get("parameters")
        .map(|p| arr(p, "parameters"))
        .unwrap_or(&[]);
    let types: Vec<String> = params
        .iter()
        .map(|p| {
            let t = p
                .get("typeDescriptions")
                .and_then(|t| t.get("typeString"))
                .and_then(Value::as_str)
                .unwrap_or("?");
            norm_type(t)
        })
        .collect();
    format!("{name}({})", types.join(","))
}

fn vis_of(v: &Value) -> Vis {
    match s(v, "visibility") {
        "external" => Vis::External,
        "public" => Vis::Public,
        "private" => Vis::Private,
        _ => Vis::Internal,
    }
}

fn base_ids(v: &Value, key: &str) -> Vec<i64> {
    arr(v, key).iter().filter_map(Value::as_i64).collect()
}

fn declared_func(m: &Value, contract: usize) -> Option<Func> {
    let (kind, name, vis, base, implemented, sig, selector) = match nt(m) {
        "FunctionDefinition" => {
            let kind = match s(m, "kind") {
                "constructor" => FKind::Constructor,
                "receive" => FKind::Receive,
                "fallback" => FKind::Fallback,
                _ => FKind::Function,
            };
            let name = s(m, "name").to_string();
            (
                kind,
                name.clone(),
                vis_of(m),
                base_ids(m, "baseFunctions"),
                flag(m, "implemented", true),
                signature(&name, m),
                parse_selector(m),
            )
        }
        "ModifierDefinition" => {
            let name = s(m, "name").to_string();
            (
                FKind::Modifier,
                name.clone(),
                Vis::Internal,
                base_ids(m, "baseModifiers"),
                m.get("body").is_some_and(|b| !b.is_null()),
                signature(&name, m),
                None,
            )
        }
        "VariableDeclaration" if flag(m, "stateVariable", false) && s(m, "visibility") == "public" => {
            let name = s(m, "name").to_string();
            (
                FKind::Getter,
                name.clone(),
                Vis::Public,
                base_ids(m, "baseFunctions"),
                true,
                format!("{name}()"),
                parse_selector(m),
            )
        }
        _ => return None,
    };
    Some(Func {
        id: id(m)?,
        name,
        sig,
        selector,
        kind,
        vis,
        contract,
        base,
        implemented,
        calls: Vec::new(),
        mismatched: 0,
    })
}

/// What a body walk collects.
#[derive(Default)]
struct Found {
    calls: Vec<RawCall>,
    mismatched: u32,
}

pub fn build(sources: &[Source]) -> Index {
    let mut ix = Index::default();
    let mut bodies: Vec<(usize, &Value)> = Vec::new();

    for src in sources {
        let mut free_ctx: Option<usize> = None;
        for node in arr(&src.ast, "nodes") {
            match nt(node) {
                "ContractDefinition" => {
                    let kind = match s(node, "contractKind") {
                        "interface" => CKind::Interface,
                        "library" => CKind::Library,
                        _ => CKind::Contract,
                    };
                    let c = ix.contracts.len();
                    let cid = id(node).unwrap_or(-1);
                    ix.contracts.push(Contract {
                        name: s(node, "name").to_string(),
                        path: src.path.clone(),
                        kind,
                        is_abstract: flag(node, "abstract", false),
                        lin_ids: base_ids(node, "linearizedBaseContracts"),
                        lin: Vec::new(),
                        funcs: Vec::new(),
                    });
                    if ix.contract_by_id.insert(cid, c).is_some() {
                        ix.collisions += 1;
                    }
                    for m in arr(node, "nodes") {
                        add_func(&mut ix, &mut bodies, declared_func(m, c), m);
                    }
                }
                "FunctionDefinition" => {
                    let c = *free_ctx.get_or_insert_with(|| {
                        let c = ix.contracts.len();
                        let name = src.path.rsplit('/').next().unwrap_or(&src.path).to_string();
                        ix.contracts.push(Contract {
                            name,
                            path: src.path.clone(),
                            kind: CKind::Free,
                            is_abstract: false,
                            lin_ids: Vec::new(),
                            lin: Vec::new(),
                            funcs: Vec::new(),
                        });
                        c
                    });
                    add_func(&mut ix, &mut bodies, declared_func(node, c), node);
                }
                _ => {}
            }
        }
    }

    for src in sources {
        collect_decl_ids(&src.ast, &mut ix.decl_ids);
    }

    for c in 0..ix.contracts.len() {
        let mut lin: Vec<usize> = ix.contracts[c]
            .lin_ids
            .iter()
            .filter_map(|i| ix.contract_by_id.get(i).copied())
            .collect();
        if lin.first() != Some(&c) {
            lin.retain(|&x| x != c);
            lin.insert(0, c);
        }
        ix.contracts[c].lin = lin;
    }

    let found: Vec<(usize, Found)> = bodies
        .iter()
        .map(|&(f, node)| {
            let mut found = Found::default();
            match nt(node) {
                "FunctionDefinition" => {
                    for m in arr(node, "modifiers") {
                        let name = m.get("modifierName");
                        let target = name
                            .and_then(refd)
                            .and_then(|r| ix.func_by_id.get(&r).copied());
                        if let Some(t) = target
                            && ix.funcs[t].kind == FKind::Modifier
                        {
                            found.calls.push(RawCall::Modifier(t));
                            let used = name.map_or("", |n| s(n, "name"));
                            if used.rsplit('.').next() != Some(ix.funcs[t].name.as_str()) {
                                found.mismatched += 1;
                            }
                        }
                        walk(&ix, m, &mut found);
                    }
                    if let Some(body) = node.get("body") {
                        walk(&ix, body, &mut found);
                    }
                }
                "ModifierDefinition" => {
                    if let Some(body) = node.get("body") {
                        walk(&ix, body, &mut found);
                    }
                }
                _ => {}
            }
            (f, found)
        })
        .collect();
    for (f, found) in found {
        ix.funcs[f].calls = found.calls;
        ix.funcs[f].mismatched = found.mismatched;
    }
    ix
}

fn collect_decl_ids(v: &Value, out: &mut HashSet<i64>) {
    match v {
        Value::Object(map) => {
            if matches!(
                nt(v),
                "ContractDefinition"
                    | "FunctionDefinition"
                    | "ModifierDefinition"
                    | "VariableDeclaration"
                    | "EventDefinition"
                    | "ErrorDefinition"
                    | "StructDefinition"
                    | "EnumDefinition"
                    | "EnumValue"
                    | "UserDefinedValueTypeDefinition"
            ) && let Some(i) = id(v)
            {
                out.insert(i);
            }
            for child in map.values() {
                collect_decl_ids(child, out);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_decl_ids(child, out);
            }
        }
        _ => {}
    }
}

fn add_func<'a>(ix: &mut Index, bodies: &mut Vec<(usize, &'a Value)>, f: Option<Func>, node: &'a Value) {
    let Some(f) = f else { return };
    let idx = ix.funcs.len();
    let c = f.contract;
    if ix.func_by_id.insert(f.id, idx).is_some() {
        ix.collisions += 1;
    }
    ix.contracts[c].funcs.push(idx);
    ix.funcs.push(f);
    bodies.push((idx, node));
}

fn walk(ix: &Index, v: &Value, found: &mut Found) {
    match v {
        Value::Object(map) => {
            if nt(v) == "FunctionCall" && s(v, "kind") == "functionCall" {
                classify(ix, v, found);
            }
            for child in map.values() {
                walk(ix, child, found);
            }
        }
        Value::Array(items) => {
            for child in items {
                walk(ix, child, found);
            }
        }
        _ => {}
    }
}

fn classify(ix: &Index, call: &Value, found: &mut Found) {
    let Some(mut callee) = call.get("expression") else {
        return;
    };
    while nt(callee) == "FunctionCallOptions" {
        match callee.get("expression") {
            Some(inner) => callee = inner,
            None => return,
        }
    }
    match nt(callee) {
        "Identifier" => {
            let r = refd(callee);
            match r.and_then(|r| ix.func_by_id.get(&r).copied()) {
                // A function-typed public variable: the target is unknown.
                Some(f) if ix.funcs[f].kind == FKind::Getter => {}
                Some(f) => {
                    if ix.funcs[f].name != s(callee, "name") {
                        found.mismatched += 1;
                    }
                    found.calls.push(RawCall::Internal(f));
                }
                None => {
                    if r.is_some_and(|r| r > 0 && !ix.decl_ids.contains(&r)) {
                        found.mismatched += 1;
                    }
                }
            }
        }
        "MemberAccess" => {
            let member = s(callee, "memberName");
            let r = refd(callee);
            match r.and_then(|r| ix.func_by_id.get(&r).copied()) {
                Some(f) => {
                    if ix.funcs[f].name != member {
                        found.mismatched += 1;
                    }
                    let recv = callee.get("expression");
                    let call = match recv {
                        Some(recv) if nt(recv) == "Identifier" => {
                            if s(recv, "name") == "super" {
                                RawCall::Super(f)
                            } else if s(recv, "name") == "this" {
                                RawCall::This(f)
                            } else if refd(recv).is_some_and(|r| ix.contract_by_id.contains_key(&r)) {
                                RawCall::Specific(f)
                            } else {
                                RawCall::External(f)
                            }
                        }
                        _ => RawCall::External(f),
                    };
                    found.calls.push(call);
                }
                None => {
                    if r.is_some_and(|r| r > 0 && !ix.decl_ids.contains(&r)) {
                        found.mismatched += 1;
                    }
                }
            }
        }
        "NewExpression" => {
            let type_name = callee.get("typeName");
            let target = type_name
                .and_then(|t| t.get("pathNode").and_then(refd).or_else(|| refd(t)))
                .and_then(|r| ix.contract_by_id.get(&r).copied());
            if let Some(c) = target {
                found.calls.push(RawCall::New(c));
            }
        }
        "ElementaryTypeNameExpression" => {}
        _ => {}
    }
}
