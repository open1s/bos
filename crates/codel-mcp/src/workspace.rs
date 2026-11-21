//! Host context, MCP roots, and the lazy workspace index.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use codel::{
    CIncludeResolver, CIncludeSymbolResolver, CSharpReceiverResolver, CSharpResolver,
    CSharpSymbolResolver, CodeGraph, CppSymbolResolver, GoReceiverResolver, GoResolver,
    GoSymbolResolver, GrammarProvider, GraphStore, IgnorePolicy, JavaReceiverResolver,
    JavaResolver, JavaScriptResolver, JavaScriptSymbolResolver, JavaSymbolResolver,
    KotlinReceiverResolver, KotlinResolver, KotlinSymbolResolver, LanguageProvider,
    LanguageResolver, NodeId, PhpResolver, PhpSymbolResolver, PythonProvider, RelationFact,
    RubyResolver, RubySymbolResolver, RustProvider, ScalaReceiverResolver, ScalaResolver,
    ScalaSymbolResolver, ScanBudget, ScanLimits, SourceIndexStatus, SymbolResolver, SyntaxFactKind,
    TypeScriptProvider, TypeScriptReceiverResolver, WorkspaceDescriptor, WorkspaceFs, WorkspaceId,
    build_symbol_candidates, content_manifest_digest, index_workspace, scan_workspace,
};
use serde_json::Value;

/// Identity and optional root supplied by the MCP host.
#[derive(Clone, Debug)]
pub struct HostContext {
    /// Host-owned checkout identity. Never derived from a path.
    pub workspace_id: WorkspaceId,
    /// Optional host-selected root. When absent, a single MCP root is used.
    pub root: Option<PathBuf>,
    /// Optional durable graph-store root.
    pub store_root: Option<PathBuf>,
}

/// One indexed workspace and its candidate graph.
#[derive(Debug)]
pub struct WorkspaceGraph {
    /// Canonical workspace descriptor.
    pub descriptor: WorkspaceDescriptor,
    /// Built graph.
    pub graph: CodeGraph,
    /// Source-index coverage; None when the graph came from the store.
    pub status: Option<SourceIndexStatus>,
    /// Whether the graph was loaded from the durable store.
    pub loaded_from_store: bool,
    /// Total source bytes observed in the inventory.
    pub raw_bytes: usize,
    /// Successfully parsed files.
    pub parsed_files: u64,
    /// Files without a matching provider.
    pub unsupported_files: u64,
    /// Files parsed with recovery.
    pub recovered_files: u64,
    /// Files that failed to load or parse.
    pub failed_files: u64,
    /// Declaration kind/type facts keyed by graph node id.
    pub symbol_types: BTreeMap<String, String>,
}

/// Read host context from initialize params.
///
/// The canonical location is
/// `params._meta.codel = { "workspaceId": "...", "root": "..." }`.
/// Top-level `workspaceId`/`root` are accepted for local tests and simple
/// clients, but BSH must use the meta object.
pub fn parse_host_context(params: &Value) -> Result<Option<HostContext>, String> {
    let container = params
        .get("_meta")
        .and_then(|meta| meta.get("codel"))
        .unwrap_or(params);
    let Some(id_value) = container
        .get("workspaceId")
        .or_else(|| container.get("workspace_id"))
    else {
        return Ok(None);
    };
    let id_text = id_value
        .as_str()
        .ok_or_else(|| "host context workspaceId must be a string".to_owned())?;
    let workspace_id = id_text
        .parse()
        .map_err(|_| format!("host context workspaceId is not a UUID: {id_text}"))?;
    let root = container
        .get("root")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let store_root = container
        .get("storeRoot")
        .or_else(|| container.get("store_root"))
        .and_then(Value::as_str)
        .map(PathBuf::from);
    Ok(Some(HostContext {
        workspace_id,
        root,
        store_root,
    }))
}

/// Read a standalone host context from environment variables.
///
/// Initialize metadata overrides this. Used by both stdio and HTTP launchers.
#[must_use]
pub fn host_from_env() -> Option<HostContext> {
    let workspace_id = std::env::var("CODEL_WORKSPACE_ID").ok()?;
    let workspace_id = workspace_id.parse().ok()?;
    Some(HostContext {
        workspace_id,
        root: std::env::var_os("CODEL_ROOT").map(PathBuf::from),
        store_root: std::env::var_os("CODEL_STORE_ROOT").map(PathBuf::from),
    })
}

/// Build the index and candidate graph for one root.
pub fn load_workspace(
    host: &HostContext,
    root: &Path,
    scip_index: Option<&Path>,
) -> Result<WorkspaceGraph, String> {
    let descriptor = WorkspaceDescriptor::new(host.workspace_id, root)
        .map_err(|error| format!("workspace root rejected: {error}"))?;
    let filesystem = WorkspaceFs::open(&descriptor)
        .map_err(|error| format!("workspace open failed: {error}"))?;
    let budget = ScanBudget::new(ScanLimits::default());
    let scip_digest = scip_index.map(scip_digest).transpose()?.unwrap_or_default();

    if let Some(store_root) = &host.store_root {
        let scan = scan_workspace(&filesystem, &budget);
        let digest = content_manifest_digest(&filesystem, &scan)
            .map_err(|error| format!("content manifest failed: {error}"))?;
        let loaded = GraphStore::new(store_root, host.workspace_id).load_with_metadata();
        match loaded {
            Ok(Some((graph, stored, metadata)))
                if stored == digest
                    && metadata.get("scip_digest").map(String::as_str)
                        == Some(scip_digest.as_str()) =>
            {
                let symbol_types = metadata
                    .get("symbol_types")
                    .and_then(|value| serde_json::from_str(value).ok())
                    .unwrap_or_default();
                return Ok(WorkspaceGraph {
                    descriptor,
                    graph,
                    status: None,
                    loaded_from_store: true,
                    raw_bytes: inventory_bytes(&scan),
                    parsed_files: 0,
                    unsupported_files: 0,
                    recovered_files: 0,
                    failed_files: 0,
                    symbol_types,
                });
            }
            Ok(Some(_)) => {
                eprintln!("codel-mcp: graph store is stale; re-indexing");
            }
            Ok(None) => {}
            Err(error) => eprintln!("codel-mcp: graph store load failed: {error}"),
        }
    }
    let rust = RustProvider;
    let typescript = TypeScriptProvider;
    let python = PythonProvider;
    let mut providers: Vec<&dyn LanguageProvider> = vec![&rust, &typescript, &python];
    for provider in &GrammarProvider::ALL {
        providers.push(provider);
    }
    let report = index_workspace(&filesystem, &budget, &IgnorePolicy::default(), &providers);
    let candidates = build_symbol_candidates(host.workspace_id, &report)
        .map_err(|error| format!("symbol candidates failed: {error}"))?;
    let mut relations: Vec<(PathBuf, RelationFact)> = Vec::new();
    for file in &report.files {
        if let Some(parse) = &file.parse {
            for relation in &parse.relations {
                relations.push((file.path.clone(), relation.clone()));
            }
        }
    }
    let inventory: BTreeSet<PathBuf> = report.files.iter().map(|file| file.path.clone()).collect();
    let java = JavaResolver;
    let javascript = JavaScriptResolver;
    let includes = CIncludeResolver;
    let go = GoResolver;
    let csharp = CSharpResolver;
    let ruby = RubyResolver;
    let php = PhpResolver;
    let kotlin = KotlinResolver;
    let scala = ScalaResolver;
    let mut imports = Vec::new();
    imports.extend(java.resolve(&report.files, &inventory));
    imports.extend(javascript.resolve(&report.files, &inventory));
    imports.extend(includes.resolve(&report.files, &inventory));
    imports.extend(go.resolve(&report.files, &inventory));
    imports.extend(csharp.resolve(&report.files, &inventory));
    imports.extend(ruby.resolve(&report.files, &inventory));
    imports.extend(php.resolve(&report.files, &inventory));
    imports.extend(kotlin.resolve(&report.files, &inventory));
    imports.extend(scala.resolve(&report.files, &inventory));
    let java_symbols = JavaSymbolResolver;
    let javascript_symbols = JavaScriptSymbolResolver;
    let kotlin_symbols = KotlinSymbolResolver;
    let scala_symbols = ScalaSymbolResolver;
    let php_symbols = PhpSymbolResolver;
    let go_symbols = GoSymbolResolver;
    let csharp_symbols = CSharpSymbolResolver;
    let ruby_symbols = RubySymbolResolver;
    let c_symbols = CIncludeSymbolResolver;
    let cpp_symbols = CppSymbolResolver;
    let java_receivers = JavaReceiverResolver;
    let typescript_receivers = TypeScriptReceiverResolver;
    let csharp_receivers = CSharpReceiverResolver;
    let kotlin_receivers = KotlinReceiverResolver;
    let scala_receivers = ScalaReceiverResolver;
    let go_receivers = GoReceiverResolver;
    let mut symbol_edges = Vec::new();
    symbol_edges.extend(java_symbols.resolve(&report.files, &candidates, &imports));
    symbol_edges.extend(javascript_symbols.resolve(&report.files, &candidates, &imports));
    symbol_edges.extend(kotlin_symbols.resolve(&report.files, &candidates, &imports));
    symbol_edges.extend(scala_symbols.resolve(&report.files, &candidates, &imports));
    symbol_edges.extend(php_symbols.resolve(&report.files, &candidates, &imports));
    symbol_edges.extend(go_symbols.resolve(&report.files, &candidates, &imports));
    symbol_edges.extend(csharp_symbols.resolve(&report.files, &candidates, &imports));
    symbol_edges.extend(ruby_symbols.resolve(&report.files, &candidates, &imports));
    symbol_edges.extend(c_symbols.resolve(&report.files, &candidates, &imports));
    symbol_edges.extend(cpp_symbols.resolve(&report.files, &candidates, &imports));
    symbol_edges.extend(java_receivers.resolve(&report.files, &candidates, &imports));
    symbol_edges.extend(typescript_receivers.resolve(&report.files, &candidates, &imports));
    symbol_edges.extend(csharp_receivers.resolve(&report.files, &candidates, &imports));
    symbol_edges.extend(kotlin_receivers.resolve(&report.files, &candidates, &imports));
    symbol_edges.extend(scala_receivers.resolve(&report.files, &candidates, &imports));
    symbol_edges.extend(go_receivers.resolve(&report.files, &candidates, &imports));
    let scip_index = scip_index.map(Path::to_path_buf).or_else(|| {
        let marker = root.join(".codel/scip-index");
        let stored = std::fs::read_to_string(marker).ok()?;
        let stored = PathBuf::from(stored.trim());
        Some(if stored.is_absolute() {
            stored
        } else {
            root.join(stored)
        })
    });
    if let Some(index) = scip_index
        && let Ok(references) = codel::load_scip_index(&index, root)
    {
        symbol_edges.extend(codel::scip_symbol_edges(&references, &candidates));
    }
    let mut symbol_types = BTreeMap::new();
    for file in &report.files {
        let Some(parse) = &file.parse else {
            continue;
        };
        for fact in &parse.facts {
            if !matches!(
                fact.kind,
                SyntaxFactKind::Variable | SyntaxFactKind::Field | SyntaxFactKind::MethodSignature
            ) {
                continue;
            }
            let (Some(name), Some(ty)) = (fact.name.as_deref(), fact.scope_path.first()) else {
                continue;
            };
            let Some(symbol) = candidates
                .symbols
                .iter()
                .filter(|symbol| {
                    symbol.id.path == file.path
                        && symbol.id.start_byte <= fact.range.start_byte
                        && fact.range.end_byte <= symbol.id.end_byte
                        && symbol.name == name
                })
                .min_by_key(|symbol| symbol.id.end_byte.saturating_sub(symbol.id.start_byte))
            else {
                continue;
            };
            symbol_types.insert(NodeId::symbol(&symbol.id).as_str().to_owned(), ty.clone());
        }
    }
    let graph = CodeGraph::build_with_symbols(&candidates, &relations, &imports, &symbol_edges)
        .map_err(|error| format!("candidate graph failed: {error}"))?;
    if let Some(store_root) = &host.store_root {
        let digest = content_manifest_digest(&filesystem, &report.scan)
            .map_err(|error| format!("content manifest failed: {error}"))?;
        let mut metadata = BTreeMap::new();
        metadata.insert("scip_digest".to_owned(), scip_digest);
        if let Ok(types) = serde_json::to_string(&symbol_types) {
            metadata.insert("symbol_types".to_owned(), types);
        }
        if let Err(error) = GraphStore::new(store_root, host.workspace_id)
            .save_with_digest_and_metadata(&graph, &digest, &metadata)
        {
            eprintln!("codel-mcp: graph store save failed: {error}");
        }
    }
    Ok(WorkspaceGraph {
        descriptor,
        graph,
        status: Some(report.status),
        loaded_from_store: false,
        raw_bytes: inventory_bytes(&report.scan),
        parsed_files: report.parsed_files,
        unsupported_files: report.unsupported_files,
        recovered_files: report.recovered_files,
        failed_files: report.failed_files,
        symbol_types,
    })
}

fn scip_digest(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    if path.is_dir() {
        let mut files = std::fs::read_dir(path)
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "scip")
            })
            .collect::<Vec<_>>();
        files.sort();
        for file in files {
            hasher.update(file.as_os_str().as_encoded_bytes());
            hasher.update(std::fs::read(&file).map_err(|error| error.to_string())?);
        }
    } else {
        hasher.update(std::fs::read(path).map_err(|error| error.to_string())?);
    }
    Ok(format!("{:?}", hasher.finalize()))
}

fn inventory_bytes(scan: &codel::WorkspaceScanReport) -> usize {
    scan.entries
        .iter()
        .filter(|entry| entry.kind == codel::WorkspaceEntryKind::File)
        .filter_map(|entry| entry.size_bytes)
        .sum::<u64>()
        .min(usize::MAX as u64) as usize
}

/// Extract filesystem roots from a roots/list result.
pub fn roots_from_value(result: &Value) -> Result<Vec<PathBuf>, String> {
    let roots = result
        .get("roots")
        .and_then(Value::as_array)
        .ok_or_else(|| "roots/list result has no roots array".to_owned())?;
    let mut paths = Vec::with_capacity(roots.len());
    for root in roots {
        let uri = root
            .get("uri")
            .and_then(Value::as_str)
            .ok_or_else(|| "roots/list entry has no uri".to_owned())?;
        let path = file_uri_to_path(uri)
            .ok_or_else(|| format!("roots/list uri is not a file URI: {uri}"))?;
        paths.push(path);
    }
    Ok(paths)
}

/// Convert a file:// URI to a local path.
fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let decoded = percent_decode(rest);
    #[cfg(windows)]
    {
        let trimmed = decoded.strip_prefix('/').unwrap_or(&decoded);
        let bytes = trimmed.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            return Some(PathBuf::from(trimmed));
        }
    }
    Some(PathBuf::from(decoded))
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = decode_hex(bytes[index + 1]);
            let low = decode_hex(bytes[index + 2]);
            if let (Some(high), Some(low)) = (high, low) {
                output.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn decode_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
