use crate::graph::{FKind, Graph};

fn parse_selector(arg: &str) -> Option<[u8; 4]> {
    let hex = arg.strip_prefix("0x").unwrap_or(arg);
    if hex.len() != 8 {
        return None;
    }
    let mut out = [0u8; 4];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Nodes matching `name`, `Contract.name`, `name(type,...)` or `0xselector`.
/// `Contract` matches either the declaring or the concrete contract.
pub fn find_targets(g: &Graph, arg: &str) -> Vec<u32> {
    let arg = arg.trim();
    let all = 0..g.nodes.len() as u32;
    if let Some(sel) = parse_selector(arg) {
        return all
            .filter(|&n| g.nodes[n as usize].selector == Some(sel))
            .collect();
    }
    let (head, sig) = match arg.find('(') {
        Some(i) => (&arg[..i], Some(arg[i..].replace(' ', ""))),
        None => (arg, None),
    };
    let (contract, fname) = match head.split_once('.') {
        Some((c, n)) => (Some(c), n),
        None => (None, head),
    };
    all.filter(|&n| {
        let node = &g.nodes[n as usize];
        let name_ok = match node.kind {
            FKind::Constructor => fname == "constructor",
            FKind::Receive => fname == "receive",
            FKind::Fallback => fname == "fallback",
            _ => node.name == fname,
        };
        if !name_ok {
            return false;
        }
        if let Some(c) = contract {
            let ctx = &g.contracts[node.ctx as usize];
            let decl = &g.contracts[node.decl as usize];
            if ctx.name != c && decl.name != c && ctx.display != c && decl.display != c {
                return false;
            }
        }
        match &sig {
            Some(sig) => node.sig.replace(' ', "") == format!("{fname}{sig}"),
            None => true,
        }
    })
    .collect()
}

/// Nodes whose concrete contract is `contract` (by name or display name),
/// i.e. everything the contract has, declared or inherited.
pub fn contract_functions(g: &Graph, contract: &str) -> Vec<u32> {
    let contract = contract.trim();
    (0..g.nodes.len() as u32)
        .filter(|&n| {
            let ctx = &g.contracts[g.nodes[n as usize].ctx as usize];
            ctx.name == contract || ctx.display == contract
        })
        .collect()
}
