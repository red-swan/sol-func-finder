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
