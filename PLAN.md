# solff — plan

CLI that reads Foundry AST artifacts and prints every call chain from an entry point down to a target function.

```
solff <name>... [--out-dir out] [--no-cache]
```

## MVP scope

- Rust. Input is `<out-dir>/**/<Contract>.json`, `ast` field only. No build-info, no fallback.
- If no artifact has an `ast`: print `no ASTs found under <out-dir>` and exit non-zero. No foundry.toml guidance.
- Output: indented tree to stdout. Entry points are roots, target is the leaf (marked `<-`).
- Not in MVP: reverse mode, JSON output, `--out`, include/exclude flags, `--strict`, depth limit, clipboard.
- Hardcoded file exclusions (by `ast.absolutePath`): `test/`, `script/`, `*.t.sol`, `*.s.sol`. `src/` and `lib/` are included. Lib is needed for linearization and for seeing overrides of lib hooks.
- `<name>` forms: `name` (all overloads, all contracts), `Contract.name`, `name(uint256)`, `0xselector`. Multiple args: one tree per arg with a header line.

## Crates

`clap` (derive), `serde` (derive), `serde_json` (AST only, parsed as `Value`, no typed solc model), `postcard`, `walkdir`, `anyhow`, plus a stable fixed hasher (`blake3` or `xxhash-rust`; not std's default hasher).

## Modules

`loader`, `index`, `graph` (serde types), `resolve`, `fingerprint`, `cache`, `query`, `render`, `cli`.

## Pipeline

1. **Load.** Walk the out dir, skip `build-info/`, parse JSON. Each artifact's `ast` is the whole source file, repeated per contract in that file, so dedupe by `absolutePath`. Apply exclusions. Skip artifacts with no `ast`. Skip ASTs whose `absolutePath` does not exist on disk (relative to cwd): stale artifacts from deleted sources would otherwise create phantom contracts and callers. Report skips as one summary line on stderr (e.g. `skipped 19 artifacts without ast, 8 with missing source`), not one warning per artifact.
2. **Index.** Collect `ContractDefinition` (with `linearizedBaseContracts`, `contractKind`, `abstract`), `FunctionDefinition`, `ModifierDefinition`, public state-variable getters. Global map keyed by node id. Assumes one compilation job; warn on id collisions (same id, different source path).
3. **Edges.** Walk bodies for:
   - `FunctionCall` with `Identifier` or `MemberAccess` callee, resolved via `referencedDeclaration`
   - `ModifierInvocation`
   - base-constructor calls
   - `new X()` -> X's constructor
   - library calls (`using for`)
4. **Dispatch.** Graph nodes are `(concrete contract, function)`. Expand inherited functions per concrete contract via `linearizedBaseContracts`. Resolve internal virtual calls and `super` to the most-derived override.
5. **Interface calls.** Callee declared on an interface -> link to every in-scope contract that inherits that interface and has a function with the same selector (`functionSelector` on the node). These edges are marked `~` (over-approximate).
6. **Entry points.** `external`/`public` functions, plus `receive`/`fallback`, on non-abstract contracts and libraries only. Abstract contracts and interfaces are not roots; the concrete derived contract carries inherited externals.
7. **Query.** Resolve args to target nodes. Reverse-BFS to the set of nodes that can reach a target. Entry points in that set are roots.
8. **Render.** Forward DFS from each root, restricted to the reaching set.
   - Suffix `(from X)` on any line where the declaring contract differs from the concrete contract (surfaces inherited entry points and overridden hooks).
   - `~` prefix on interface/selector-matched edges.
   - Cycle on current path: `(recursive)`, stop.
   - Subtree already printed in full: `(see above)`.
   - Overloads: print signature when ambiguous.

```
MyVault.withdraw (from ERC4626)
  MyVault._hook
Router.swap
  ~Pool.swap
    Pool._update
      Token.mint <-
```

## Diagnostics (stderr)

- N unresolved call sites in scope (`.call`, `delegatecall`, function pointers, proxies)
- id-collision warnings
- skipped-artifact summary line (no ast / source missing on disk)

Replayed on cache hits so output is identical either way.

## Cache

The cache is the resolved graph (after step 5), not rendered output. Query matching, reverse reachability, and rendering run on the in-memory graph each time; reverse adjacency is rebuilt at load.

- Location: `./.solff/graph.bin` plus `.solff/.gitignore` containing `*`. cwd is assumed to be the Foundry project root.
- Contents: nodes (contract, function name, signature, selector, visibility, declaring contract, source path, entry flag), forward edges with `~` flag, diagnostics.
- Encoding: `serde` + `postcard`. Header = magic bytes, format-version const, fingerprint. Read the header first; decode the body only on match.
- Fingerprint = hash of: sorted `(relative path, mtime, size)` of every artifact JSON (excluding `build-info/`), canonical out-dir path, solff version, format-version const. Compared for equality, not "newer" (handles deletions, renames, checkouts restoring old mtimes). Whole-graph invalidation: a change anywhere can change any query's answer.
- Flow: stat + fingerprint -> header match? decode : full pipeline then write (temp file + rename) -> query + render.
- Failure handling: missing/truncated/undecodable file = miss, rebuild silently. Failed write = one-line warning, run continues. Concurrent runs safe (atomic rename).
- `--no-cache` skips read and write.
- Hidden `--dump-graph` debug flag prints the graph as text.

## Known limits (document in `--help`)

- Unresolved dynamic calls (see Diagnostics).
- `~` edges over-approximate.
- Single-compilation-job id assumption.
- Per-concrete-contract expansion multiplies nodes in inheritance-heavy code; main driver of cache size. Intern strings only if it becomes a problem.

## Build order

1. Skeleton, clap, loader, dedupe, exclusions.
2. Index and internal-call edges (graph types serde-derived from the start).
3. Inheritance and virtual dispatch.
4. Interface and selector edges.
5. Query and render (`(from X)`, cycles, see-above).
6. Fingerprint and cache read/write in front of the pipeline.
7. Tests.

## Tests

Fixture Foundry project (needs `forge` to generate `out/`) covering: inheritance and overrides, hook override creating an inherited entry point, interface calls, modifiers, libraries, `super`, recursion, overloads, abstract/interface non-roots, excluded test/script files.

Real-world integration target: `/home/red/Code/Spearbit/client-code/Polymarket/polymarket-v2` (read-only; run solff from that directory, which creates a self-ignoring `.solff/` there). Observed facts about its `out/`:
- 792 artifacts, 309 unique source ASTs, `absolutePath` is repo-relative (e.g. `src/routers/BridgeRouter.sol`).
- Single solc (0.8.34), 0 node-id collisions across declarations.
- 19 stale artifacts with no `ast` (sources deleted, old mtimes); 8 artifacts carry an AST for a source file no longer on disk (mostly `script/` and test files, plus `script/upgrades/UpgradeLib.sol`, `SafeTxBuilderLib.sol`).
- `build-info/` files do not have a top-level `output`; ignored by design.
Use it to check: skip-summary counts match the above, no phantom contracts, `lib/` bases (OpenZeppelin, solady) are in the graph, upgradeable/proxy patterns produce an unresolved-call count, and results for a few known functions agree with Slither's call graph.

Cache tests: encode/decode round-trip; cached vs uncached stdout+stderr identical; touching, deleting, or adding an artifact forces rebuild; corrupt file falls back to rebuild.

## Later (not MVP)

reverse mode, `--json`, include/exclude flags, `--strict`, depth limit, multi-compilation-job id handling, string interning.

Potential feature: dynamic fish completion. Tab completes contract names (with a trailing `.`), then `Contract.<tab>` completes that contract's functions, via a fish completion function (`completions/solff.fish` in the fish config dir) that calls solff itself. Needs two cheap machine-readable listings (all contract names; `--list Contract` already gives the functions, but with a `(from X)` suffix that scripts must strip) and the warm cache (13ms) keeps it fast. Fish shows completions on Tab, not live as you type; the pager filters as you continue typing.

## Status: MVP implemented

All build-order steps are done; `cargo test` (14 integration tests against `tests/fixture`, a real Foundry project with committed `out/`) and `cargo clippy --all-targets` are clean.

Deviations and decisions made while building:
- **Constructors are entry points.** The most-derived constructor of each concrete contract is a root (it links to the other constructors in the linearization); `new X()` links to X's constructor root. Public state-variable getters are indexed as leaf nodes and can be entry points or targets.
- **Modifiers** are graph nodes, rendered with a `[modifier]` suffix.
- **Free functions** live under a pseudo-contract named after the file (e.g. `Lib.sol.freeHelper`).
- **Duplicate contract names** (e.g. solady and OpenZeppelin `UUPSUpgradeable`) render as `path:Name`.
- **Missing-source skipping** runs after test/script exclusion, so the summary line only counts non-excluded sources (Polymarket: `skipped 19 artifacts without ast, 0 sources missing on disk`; the 8 deleted-source ASTs are all in excluded paths).
- **`Contract.name` target** matches either the declaring or the concrete contract.
- **Loader** probes `absolutePath` on all cores via `RawValue`, then fully parses only unique sources (Polymarket cold run: 9.6s single-threaded -> 0.9s).
- Hidden `--dump-graph` prints every node and edge.
- Not followed (documented in `--help`): operator overloads, state-variable initializers, `.call`/`delegatecall`/`staticcall`, function pointers, proxies.

Polymarket results: 4355 nodes, 1247 entry nodes, 25 unresolved call sites, cold 0.9s, warm 13ms (cache 276 KB), cached and uncached output byte-identical. Solady `UUPSUpgradeable.upgradeToAndCall` shows as an entry point on each derived contract, reaching its `_authorizeUpgrade` override.

Not done: cross-check against Slither's call graph (not installed here).

## Update: hard/soft modes and root absorption

- **Two searches.** Default (hard): only chains made entirely of certain edges, no `~` ever appears. `--soft`: only chains containing at least one `~` edge, so the two outputs are disjoint. Implemented as a backward search over `(node, seen_approx)` states in `render.rs`; the cache is unchanged (edges already carry the `~` flag).
- **Absorption.** A root whose whole tree already appears as a subtree of another root's tree is not printed as its own root. Roots that reach each other keep the first by label. Consequence: an entry point that is also called from another entry point is only visible nested (e.g. `Vault.deposit` under `Vault.viaThis`).
- `(see above)` is kept for repeated subtrees within a tree.
- 16 integration tests; Polymarket `SafeTransferLib.safeTransferFrom`: 107 lines hard, 26 lines soft.

## Update: caller trees, full expansion, entry arrows

- **Top levels are the tallest trees**: nodes (states, in soft mode) that nothing else in the result calls. A group of nodes that only call each other contributes one top (an entry point if there is one, else first by label). Tops whose tree contains no entry point other than the target are dropped (never-called internals such as an unused inherited `_baseSuper`).
- **Arrows mark entry points** (` <-`) at any depth; the function of interest is always the last level and is no longer marked.
- **No `(see above)`**: a function reached by several paths is expanded in full under each caller. `(recursive)` still cuts cycles. On Polymarket the largest hard results are ~200 lines (~10ms); soft results can be much larger because guessed edges fan out (`_mint`: 1910 lines).
- **Mixed-compilation detection**: forge's incremental build rewrites only changed artifacts, so a stale `out/` can mix node-id spaces. Call sites that reference an id declared nowhere in scope, or a function with a different name, are counted and reported as one warning on stderr (import aliases can also trigger it). Detection only; results from such a build can be wrong. Test fixture `out/` is regenerated with `forge build --force`.
- Cache `FORMAT_VERSION` is 2. 18 integration tests.

## Update: tree built from the function of interest, no skip summary

- **Orientation reversed.** The top level is the function of interest; each level below is everything that calls the level above, up to the entry points (marked `<-`, at any depth). The part between the function of interest and a fork prints once, so `finalize` and `resolveResult` share one `_finalizeConditions -> _reportConditionResult -> binaryPayout` path. A `~` line calls the line above it through a guess. Only callers that lead up to an entry point are shown; recursion is cut with `(recursive)`; no `(see above)`, subtrees expand in place.
- **Several matches** (a name present in many contracts) give several top-level lines; a match that is itself a caller of another match is nested under it, not repeated.
- **Soft mode** keeps its meaning (chains containing at least one `~`), searched as `(node, seen_approx)` states.
- **Removed the "skipped N artifacts" stderr line**: stale/AST-less artifacts don't affect results, and "no ASTs at all" is already an error.
- Cache `FORMAT_VERSION` 3; 19 integration tests.

## Update: no unresolved-call-site count

The global "N unresolved call sites" stderr line is removed (not specific to a query, not actionable). Unresolved calls (`.call`/`delegatecall`/`staticcall`, function pointers, proxies) simply produce no edge, so chains through them are missing; this limit is stated in `--help`. The mixed-compilation warning stays (it signals wrong results). Cache `FORMAT_VERSION` 4.

## Update: declaring-contract names with concrete-contract annotation

`(from X)` is gone. Every line reads `Declaring.function`, where the code lives. `@Concrete` is appended when the concrete contract differs from the declaring one and either the line is an entry point or its concrete contract differs from the line above (the start of a run of calls inside one contract). Lines where declaring equals concrete never carry it. Entry points always show their concrete contract (the deployed surface). `Graph::name`/`label` build the strings; the cache `FORMAT_VERSION` is 5; 20 integration tests; the fixture gained an inherited `chain -> _mid -> _inner` path to cover the run rule.

## Update: concrete contract moves after the arrow

Supersedes the `@Concrete` annotation. Every line is `Declaring.function` (where to open the code). Entry points end with `<-` followed by the contract they are called on, omitted when it equals the declaring contract (`Base.entry <- Vault`, `OracleAggregator.finalize <-`). `Graph::label` (declaring name plus `@concrete`) remains only for sorting and `--dump-graph`. 20 integration tests; cache format unchanged.

## Update: `--entries`

`--entries` prints only the entry points that reach the function(s) of interest, one `Contract.function` per line, sorted and de-duplicated, with no tree and no arrows. The contract is the one the entry point is called on (`Vault.entry`, not `Base.entry`); overloads print their signature. Respects `--soft` (only entry points whose chain has a `~` call; the target itself is not listed there unless a guessed call reaches it). Multiple names get `# name` headers as before. Implemented as `render::entries`, sharing the renderer's reachability setup (`Renderer::new`); `Graph::concrete_name` builds the string. Cache format unchanged.

## Potential features: override lookup and visibility listing

1. **Override lookup (`--overrides`).** Given `Contract.name` (or any name form), print what the function overrides up the linearization, nearest base first, one `Declaring.function` per line, down to the root declaration (including interface declarations). Also useful in reverse: what overrides this function in derived contracts (in scope).
   - Source of truth: `baseFunctions` on each `FunctionDefinition` (ids of the functions it directly overrides; also present on modifiers and public state-variable getters). Walk it transitively. No re-derivation from names or selectors needed.
   - Multiple inheritance: `override(A, B)` yields several direct bases, so the result is a DAG, not a chain. Print as a tree, with shared ancestors repeated in place (consistent with the caller tree).
   - Mark functions that are `virtual` (can be overridden further) and ones that call `super` (the chain continues at runtime).
   - Needs `baseFunctions` stored in the cache nodes, so `FORMAT_VERSION` bumps.
   - Edge cases: an interface function implemented by a state-variable getter; free functions have no overrides; the same name in unrelated contracts must not match (use ids, not names).

2. **List by visibility/mutability (`--list` filters).** Extend `--list Contract` with filters: visibility (`external`, `public`, `internal`, `private`) and state mutability (`pure`, `view`, `nonpayable`, `payable`). Flags combinable, e.g. `--list Vault --visibility internal --mutability pure`. Output stays one `Declaring.function` per line, sorted, so it is script-friendly (also feeds fish completion).
   - Visibility is already stored on cache nodes; mutability is not, so add it (`FORMAT_VERSION` bump).
   - Decide whether inherited functions are included by default (`--list` currently shows them); add an own-only switch if needed.
   - Modifiers have no mutability; list separately or only under a `--modifiers` switch.

3. **Origin lookup (`--origin`).** Given `Contract.name`, print the contract that declares the implementation `Contract.name` actually runs, with source path and line, e.g. `Vault.deposit` -> `ERC4626.deposit (lib/solady/src/tokens/ERC4626.sol:123)`. Answers "A calls F, where does F come from".
   - Already known to the graph: nodes are `(concrete contract, function)` and carry the declaring contract and source path. Lookup is a node match plus printing the declaring side; no new traversal.
   - Ambiguity to settle: if `A.F` is declared in `A` and overrides `B.F`, "origin" is `A` (the code that runs). The chain to `B` belongs to `--overrides`; print a hint line or combine the two (`--origin` = first line of `--overrides`).
   - Without a contract (`F` alone): one line per concrete contract that has `F`, grouped by declaring contract, so many contracts inheriting the same base collapse to one entry with a count.
   - Covers: inherited functions, modifiers, public state-variable getters, library functions via `using for`, free functions (pseudo-contract named after the file).
   - Needs source line: store the `src` offset in cache nodes and convert to a line (read the file, count newlines), `FORMAT_VERSION` bump.

4. **Subcommands instead of flags.** Replace mode flags with clap subcommands (`#[derive(Subcommand)]`). Sketch:
   - `solff callers <name>...` (current default behavior; `--soft`, `--no-cache`, `--out-dir` stay as options)
   - `solff entries <name>...` (was `--entries`)
   - `solff list <Contract> [visibility] [mutability]` (was `--list`)
   - `solff overrides <Contract.name>`
   - `solff origin <Contract.name>`
   - Shared options (`--out-dir`, `--no-cache`) become global args on the top-level command.
   - Decide the bare form: `solff <name>` currently means callers. Either keep it as an alias via a default subcommand (clap `args_conflicts_with_subcommands` or an external-subcommand fallback), or make it a breaking change to `solff callers`. Name collisions are the risk: a function named `list` or `entries` would be parsed as a subcommand.
   - Filters for `list` as positional/value words (`solff list Vault internal pure`) are possible but ambiguous with contract names; keep `--visibility`/`--mutability` as options unless the word sets are disjoint from Solidity identifiers (they are keywords, so they cannot collide with function or contract names).
   - Touches `cli.rs`, `tests/cli.rs`, `--help` text, and the fish completion sketch (completions per subcommand). No cache change.

5. **Call-site resolver (`solff goto`).** Go-to-definition for a call you are reading. `solff goto MultiOutcomeModule:161 encodeFromData` prints the declaration(s) that call resolves to, as `path:line` plus `Declaring.function(signature)`, so you can jump straight to the right overload instead of text-searching a name that exists in many places.
   - Input: `<Contract|path>:<line> [name]`. Contract names resolve as elsewhere (`path:Name` for duplicates). The optional name filters when a line holds several calls; without it, print every call on that line, one per line, in source order.
   - Resolution: find `FunctionCall`/`ModifierInvocation` nodes in the file whose `src` range starts on that line, take `referencedDeclaration`, print the declaration's file and line. This is the exact target solc resolved, so overloads and `using for` library calls are correct, unlike a name search.
   - Virtual calls: print the declaration the call names, then, if the enclosing contract (or a derived concrete one) overrides it, the dispatched implementation(s) on following lines, reusing the graph's dispatch. Interface calls list the `~` guessed implementations, marked as guesses.
   - Needs call-site source ranges in the graph: store `src` on edges (or a per-node call-site list) and the declaration's `src` on nodes, plus offset-to-line conversion (shared with `--origin`). `FORMAT_VERSION` bump. Alternative with no cache change: parse the one file's AST on demand, since the query is single-file; the cache is only needed for the dispatch part.
   - Edge cases: calls with no `referencedDeclaration` (`.call`, function pointers, casts, struct constructors, events) print `unresolved` with the reason; line numbers are of the on-disk source, which may differ from the AST if the file changed since the build (warn when the file mtime is newer than the artifact).

6. **Origin-qualified listing (`--list` output change or `--list-origin`).** List every function of a contract (declared and inherited) as `Declaring.function`, so the inheritance source of each function is visible at a glance, e.g. for `Vault`: `ERC4626.deposit`, `Ownable.owner`, `Vault.withdraw`. Sorted, one per line.
   - Today `--list` prints `deposit (from ERC4626)` / bare `withdraw` for own functions. The `Declaring.function` form replaces the suffix; own functions get the contract prefix too, so every line has the same shape. Also fixes the fish-completion sketch (no `(from X)` to strip).
   - Decide: change `--list` outright (breaking, simpler) or add a flag. Recommended: change `--list`; it is pre-release.
   - Optional grouping (`--group`): print a `# Declaring` header per declaring contract in linearization order (most derived first) instead of a flat sorted list, which reads as the inheritance layout.
   - Overridden functions: only the most derived implementation is listed (current dedupe by signature and kind), shown under the contract that declares it. Overloads print their signature.
   - Covers modifiers, public state-variable getters, and constructors the same way as today's `--list`; free functions use the file pseudo-contract.
   - No cache change: `Graph::name`/`inherited_ctx` already supply declaring and concrete contracts. Touches `cli.rs` list branch and `tests/cli.rs`.

## Potential feature: look down (callees)

`solff` only answers "who calls this function" (up, to entry points). Add the other direction: "what does this function call" (down, to the leaves).

- Given a function, print the tree of everything it calls, transitively, down to functions that call nothing further in scope (external library calls, interfaces with no in-scope implementation, builtins).
- Forward edges are already in the graph (`adj`), so this is a forward DFS from the target nodes; no new indexing. The cache is unchanged.
- Reuse the caller-tree conventions: `Declaring.function` lines, `~` for guessed (interface/selector) edges, `(recursive)` to cut cycles, hard vs `--soft`, and the same name forms (`name`, `Contract.name`, `name(type)`, `0xselector`).
- Decide the flag: `--down` (callers stay the default), or the subcommand split (`solff callers` / `solff callees`).
- Decide leaf marking: the caller tree marks entry points with `<-`; a callee tree needs its own leaf rule (for example mark external calls out of scope, or state writes).
- Fan-out is the risk: callee trees grow much larger than caller trees (soft mode especially). Consider a depth limit (`--depth`) and a `--entries`-style flat form that lists only the distinct leaf functions reached.
- Respect `--include` / `--exclude` on the printed lines. The graph itself stays unfiltered.

## Potential feature: functions that modify a state variable

Name a state variable and list every function that writes it, directly or through callees.

- **Reuse.** The functions that write the variable directly become target nodes. Everything else already exists: the caller tree shows what reaches them, and `--entries` lists the external functions that can end up modifying the variable. Variable identity is the declaration id (`referencedDeclaration` on each `Identifier`), the same mechanism calls use. A variable declared in a base contract has one id across all derived contracts, so inheritance needs no special handling.
- **New work.**
  1. Index every state variable (id, name, declaring contract). Today only public ones are indexed, as getter nodes. About 20 lines.
  2. Record direct writes per function while walking bodies: `Assignment` (including compound and tuple), `UnaryOperation` with `++`, `--` or `delete`, and `push`/`pop` on the variable. Unwrap the left side through `IndexAccess` and `MemberAccess` chains to the base `Identifier` and check whether it is a state variable. Store the written variable ids on each node (cache `FORMAT_VERSION` bump). About 100 to 150 lines.
  3. CLI: a way to name the variable (`--writes Contract.var`, or `var`) and choose the output (direct writers, caller tree, or `--entries`).
- **Hard cases (precision).**
  - Local storage pointers: `Position storage p = positions[id]; p.size = 0;` writes the variable through an alias. Common in real code; needs alias tracking for locals initialised from a state variable.
  - Storage parameters: passing a state variable to an internal function that writes it, and library calls such as `EnumerableSet.add(set, x)`.
  - Inline assembly `sstore` cannot be tied to a variable by name. Flag the function as "may write unknown storage" instead of resolving it.
  - `delegatecall` is already an unresolved edge and can write storage in unseen code.
  - For the guessed cases, mark the result with the existing `~` so missed writers and extra writers stay visible.
- **Estimate.** Direct writes only: about a day. Adding storage aliases and storage parameters roughly doubles it, and decides whether the output is trustworthy for an audit.
- **Mirror feature.** "Which functions read this variable" comes almost free once identifier resolution exists: every `Identifier` that references the variable and is not a write target.
- **Suggested first cut.** Direct writes plus local storage-pointer aliases, `sstore` and storage-parameter cases as `~` guesses, exposed through the existing tree and `--entries` outputs.

## Potential feature: immediate callers (`--depth`)

Show only the callers one step above the function of interest instead of the whole tree up to the entry points.

- Flag: `--depth N` (`--depth 1` = direct callers only). Default stays unlimited. Also applies to the callee-tree feature if that lands.
- The graph has everything needed (`Renderer::callers`). `visit` stops recursing at depth N. A node cut off by the limit that has callers above it is marked (for example `...`) so a truncated branch is not mistaken for a leaf.
- Decide whether depth-limited output still requires chains to reach an entry point. The tree drops callers that no entry point reaches. A depth-limited view probably should keep that rule so the result is a prefix of the full tree. An option to include unreachable direct callers would match what `--dump-graph` shows today.
- Interaction with `--soft` sections: a chain is filed by its loosest edge, which may lie above the cut. Keep filing by the full chain (consistent with the full tree), or by the visible part only. Prefer the full chain.
- `--entries` ignores depth (it lists entry points, not callers).
- Stopgap today: `solff --dump-graph | awk '/^[^ ]/{cur=$0} /^    -> ~*Target\.fn/{print cur}'` lists every direct caller, reachable or not.

## Update: `--soft` sections by how the guess was made

Edges carry a kind (`EdgeKind`, replacing `approx: bool`): `Certain`, `Iface`, `Encoded`, `Selector`, tightest to loosest. A chain is filed under its loosest edge; `--soft` prints one headed section (`## ...`) per non-empty kind, `--entries --soft` the same. Hard mode is unchanged (certain chains only). Cache `FORMAT_VERSION` 8.

- **`~` Iface**: ordinary call on an interface or base type; callee is any in-scope implementer (as before).
- **`~~` Encoded**: `abi.encodeCall(X.f, ..)` and `abi.encodeWithSelector(X.f.selector, ..)`. `X.f` is resolved by declaration id, then expanded like an interface call (implementers of the declaring contract), even when `X` is a concrete contract, because the encoded bytes do not say who receives them. These calls are usually followed by an assembly or low-level `staticcall`, which is why they were invisible.
- **`~~~` Selector**: matched by selector value only: contracts outside the declaring hierarchy that have a function with the same selector (previously folded into `~`), plus selectors given as values: a 4-byte hex literal, `bytes4(..)` of one, `bytes4(keccak256("f(address)"))`, `abi.encodeWithSignature("f(address)")` (keccak via `sha3`).
- Per callee the tightest kind wins (edge dedup sorts by kind). Search state is `(node, level)` with level = loosest edge from the node down; one search per section goal.
- Not resolved: a selector held in a constant or variable (`bytes4 constant SEL = ..; abi.encodeWithSelector(SEL, ..)`), `abi.encodePacked` and hand-built calldata, selectors from `msg.sig`.
- Fixture: `tests/fixture/src/Encoded.sol` (`Encoder`, `Pinger`, `Unrelated`, `Mixed`), 3 new tests.
