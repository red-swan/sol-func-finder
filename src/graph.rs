use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum CKind {
    Contract,
    Interface,
    Library,
    /// Pseudo-contract holding the free functions of one source file.
    Free,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum FKind {
    Function,
    Constructor,
    Receive,
    Fallback,
    Modifier,
    Getter,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Vis {
    Public,
    External,
    Internal,
    Private,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContractInfo {
    pub name: String,
    pub display: String,
    pub path: String,
    pub kind: CKind,
    pub is_abstract: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    /// Concrete contract the function executes under.
    pub ctx: u32,
    /// Contract that declares the function.
    pub decl: u32,
    pub name: String,
    pub sig: String,
    pub selector: Option<[u8; 4]>,
    pub kind: FKind,
    pub vis: Vis,
    pub entry: bool,
    /// Another effective function with the same name exists in `ctx`.
    pub overloaded: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Edge {
    pub to: u32,
    /// Interface / selector-matched edge (over-approximation).
    pub approx: bool,
}

#[derive(Default, Debug, Serialize, Deserialize)]
pub struct Graph {
    pub contracts: Vec<ContractInfo>,
    pub nodes: Vec<Node>,
    pub adj: Vec<Vec<Edge>>,
    /// Stderr lines, replayed on every run (including cache hits).
    pub diag: Vec<String>,
}

impl Graph {
    /// `Declaring.function`: where the code lives.
    pub fn name(&self, n: u32) -> String {
        let node = &self.nodes[n as usize];
        let decl = &self.contracts[node.decl as usize].display;
        let base = match node.kind {
            FKind::Constructor => "constructor".to_string(),
            FKind::Receive => "receive".to_string(),
            FKind::Fallback => "fallback".to_string(),
            _ if node.overloaded => node.sig.clone(),
            _ => node.name.clone(),
        };
        let mut s = format!("{decl}.{base}");
        if node.kind == FKind::Modifier {
            s.push_str(" [modifier]");
        }
        s
    }

    /// `Concrete.function`: the contract the function is called on, not where it is declared.
    pub fn concrete_name(&self, n: u32) -> String {
        let name = self.name(n);
        let decl = &self.contracts[self.nodes[n as usize].decl as usize].display;
        let ctx = &self.contracts[self.nodes[n as usize].ctx as usize].display;
        match name.strip_prefix(decl.as_str()) {
            Some(rest) => format!("{ctx}{rest}"),
            None => name,
        }
    }

    /// The concrete contract the function runs under, when it is not the declaring one.
    pub fn inherited_ctx(&self, n: u32) -> Option<&str> {
        let node = &self.nodes[n as usize];
        (node.ctx != node.decl).then(|| self.contracts[node.ctx as usize].display.as_str())
    }

    /// `Declaring.function @Concrete`, for sorting and debugging.
    pub fn label(&self, n: u32) -> String {
        match self.inherited_ctx(n) {
            Some(ctx) => format!("{} @{ctx}", self.name(n)),
            None => self.name(n),
        }
    }
}
