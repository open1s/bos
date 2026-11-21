//! Per-language symbol-scope binding over import edges.
//!
//! Import resolution answers "which file". This layer answers "which symbol is
//! visible under which local name in this file", then binds type references and
//! calls to those imported symbols. It is still candidate-only: an ambiguous or
//! unresolved name produces no edge.

use std::collections::BTreeMap;
use std::path::Path;

use crate::classification::LanguageId;
use crate::index::SourceFileResult;
use crate::language_resolver::ImportEdge;
use crate::resolution::{ReferenceKind, SymbolCandidateReport, SymbolId, SymbolRecord};
use crate::syntax::{SourceRange, SyntaxFactKind};

/// One scoped symbol binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopedSymbolEdge {
    /// Enclosing declaration in the importing file.
    pub from: SymbolId,
    /// Imported declaration.
    pub to: SymbolId,
    /// Reference category.
    pub kind: ReferenceKind,
    /// Source spelling.
    pub spelling: String,
    /// Source range.
    pub range: SourceRange,
}

/// One language's symbol-scope resolver.
pub trait SymbolResolver: Send + Sync {
    /// Language this resolver understands.
    fn language(&self) -> LanguageId;

    /// Resolve imported aliases and bind references to imported symbols.
    fn resolve(
        &self,
        files: &[SourceFileResult],
        symbols: &SymbolCandidateReport,
        imports: &[ImportEdge],
    ) -> Vec<ScopedSymbolEdge>;
}

/// Java import-scope resolver.
#[derive(Clone, Copy, Debug, Default)]
pub struct JavaSymbolResolver;

impl SymbolResolver for JavaSymbolResolver {
    fn language(&self) -> LanguageId {
        LanguageId::Java
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        symbols: &SymbolCandidateReport,
        imports: &[ImportEdge],
    ) -> Vec<ScopedSymbolEdge> {
        resolve_qualified(files, symbols, imports, LanguageId::Java)
    }
}

/// Kotlin import-scope resolver.
#[derive(Clone, Copy, Debug, Default)]
pub struct KotlinSymbolResolver;

impl SymbolResolver for KotlinSymbolResolver {
    fn language(&self) -> LanguageId {
        LanguageId::Kotlin
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        symbols: &SymbolCandidateReport,
        imports: &[ImportEdge],
    ) -> Vec<ScopedSymbolEdge> {
        resolve_qualified(files, symbols, imports, LanguageId::Kotlin)
    }
}

/// Scala import-scope resolver.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScalaSymbolResolver;

impl SymbolResolver for ScalaSymbolResolver {
    fn language(&self) -> LanguageId {
        LanguageId::Scala
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        symbols: &SymbolCandidateReport,
        imports: &[ImportEdge],
    ) -> Vec<ScopedSymbolEdge> {
        resolve_qualified(files, symbols, imports, LanguageId::Scala)
    }
}

/// PHP import-scope resolver.
#[derive(Clone, Copy, Debug, Default)]
pub struct PhpSymbolResolver;

impl SymbolResolver for PhpSymbolResolver {
    fn language(&self) -> LanguageId {
        LanguageId::Php
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        symbols: &SymbolCandidateReport,
        imports: &[ImportEdge],
    ) -> Vec<ScopedSymbolEdge> {
        resolve_qualified(files, symbols, imports, LanguageId::Php)
    }
}

/// JavaScript named-import resolver.
#[derive(Clone, Copy, Debug, Default)]
pub struct JavaScriptSymbolResolver;

impl SymbolResolver for JavaScriptSymbolResolver {
    fn language(&self) -> LanguageId {
        LanguageId::JavaScript
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        symbols: &SymbolCandidateReport,
        imports: &[ImportEdge],
    ) -> Vec<ScopedSymbolEdge> {
        let mut edges = Vec::new();
        for file in files {
            if file.language != Some(LanguageId::JavaScript) {
                continue;
            }
            let mut aliases = Vec::new();
            for import in imports.iter().filter(|import| import.from == file.path) {
                for (target, local) in named_imports(&import.spelling) {
                    if let Some(symbol) = symbols
                        .symbols
                        .iter()
                        .find(|symbol| symbol.id.path == import.to && symbol.name == target)
                    {
                        aliases.push((local, symbol));
                    }
                }
            }
            let Some(parse) = &file.parse else {
                continue;
            };
            for fact in &parse.facts {
                let Some(name) = fact.name.as_deref() else {
                    continue;
                };
                let Some((_, symbol)) = aliases.iter().find(|(local, _)| *local == name) else {
                    continue;
                };
                let Some(from) = enclosing_symbol(symbols, &file.path, fact.range) else {
                    continue;
                };
                let kind = match fact.kind {
                    SyntaxFactKind::Call => ReferenceKind::FunctionCall,
                    SyntaxFactKind::MethodCall => ReferenceKind::MethodCall,
                    SyntaxFactKind::TypeReference => ReferenceKind::TypeReference,
                    _ => continue,
                };
                edges.push(ScopedSymbolEdge {
                    from: from.id.clone(),
                    to: symbol.id.clone(),
                    kind,
                    spelling: name.to_owned(),
                    range: fact.range,
                });
            }
        }
        edges
    }
}

/// Go package-scope resolver.
#[derive(Clone, Copy, Debug, Default)]
pub struct GoSymbolResolver;

impl SymbolResolver for GoSymbolResolver {
    fn language(&self) -> LanguageId {
        LanguageId::Go
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        symbols: &SymbolCandidateReport,
        imports: &[ImportEdge],
    ) -> Vec<ScopedSymbolEdge> {
        resolve_file_imports(files, symbols, imports, LanguageId::Go)
    }
}

/// C# namespace-scope resolver.
#[derive(Clone, Copy, Debug, Default)]
pub struct CSharpSymbolResolver;

impl SymbolResolver for CSharpSymbolResolver {
    fn language(&self) -> LanguageId {
        LanguageId::CSharp
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        symbols: &SymbolCandidateReport,
        imports: &[ImportEdge],
    ) -> Vec<ScopedSymbolEdge> {
        resolve_file_imports(files, symbols, imports, LanguageId::CSharp)
    }
}

/// Ruby require-scope resolver.
#[derive(Clone, Copy, Debug, Default)]
pub struct RubySymbolResolver;

impl SymbolResolver for RubySymbolResolver {
    fn language(&self) -> LanguageId {
        LanguageId::Ruby
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        symbols: &SymbolCandidateReport,
        imports: &[ImportEdge],
    ) -> Vec<ScopedSymbolEdge> {
        resolve_file_imports(files, symbols, imports, LanguageId::Ruby)
    }
}

/// Java receiver table: local/field type to member lookup.
#[derive(Clone, Copy, Debug, Default)]
pub struct JavaReceiverResolver;

impl SymbolResolver for JavaReceiverResolver {
    fn language(&self) -> LanguageId {
        LanguageId::Java
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        symbols: &SymbolCandidateReport,
        _imports: &[ImportEdge],
    ) -> Vec<ScopedSymbolEdge> {
        let mut edges = Vec::new();
        for file in files {
            if file.language != Some(LanguageId::Java) {
                continue;
            }
            let Some(parse) = &file.parse else {
                continue;
            };
            let variables: Vec<&crate::syntax::SyntaxFact> = parse
                .facts
                .iter()
                .filter(|fact| {
                    fact.kind == SyntaxFactKind::Variable
                        && fact.name.is_some()
                        && !fact.scope_path.is_empty()
                })
                .collect();
            let mut fields: BTreeMap<(String, String), String> = BTreeMap::new();
            for variable in &variables {
                let Some(owner) = enclosing_type_symbol(symbols, &file.path, variable.range) else {
                    continue;
                };
                if let (Some(name), Some(ty)) =
                    (variable.name.as_deref(), variable.scope_path.first())
                {
                    fields.insert((owner.name.clone(), name.to_owned()), ty.to_owned());
                }
            }
            let mut type_params: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for parameter in parse
                .facts
                .iter()
                .filter(|fact| fact.kind == SyntaxFactKind::TypeParameter)
            {
                if let (Some(name), Some(owner)) =
                    (parameter.name.as_deref(), parameter.scope_path.first())
                {
                    type_params
                        .entry(owner.clone())
                        .or_default()
                        .push(name.to_owned());
                }
            }
            let mut signatures: BTreeMap<(std::path::PathBuf, usize), (String, Vec<String>)> =
                BTreeMap::new();
            for signature in parse
                .facts
                .iter()
                .filter(|fact| fact.kind == SyntaxFactKind::MethodSignature)
            {
                signatures.insert(
                    (file.path.clone(), signature.range.start_byte),
                    (
                        signature.scope_path.get(1).cloned().unwrap_or_default(),
                        signature
                            .scope_path
                            .iter()
                            .skip(2)
                            .map(|ty| normalize_type(ty))
                            .collect(),
                    ),
                );
            }
            let mut returns: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for signature in parse
                .facts
                .iter()
                .filter(|fact| fact.kind == SyntaxFactKind::MethodSignature)
            {
                if let (Some(name), Some(return_type)) =
                    (signature.name.as_deref(), signature.scope_path.first())
                {
                    returns
                        .entry(name.to_owned())
                        .or_default()
                        .push(normalize_type(return_type));
                }
            }
            for call in parse
                .facts
                .iter()
                .filter(|fact| fact.kind == SyntaxFactKind::ReceiverCall)
            {
                let (Some(method), Some(receiver)) =
                    (call.name.as_deref(), call.scope_path.first())
                else {
                    continue;
                };
                let owner = enclosing_type_symbol(symbols, &file.path, call.range);
                let Some(ty) = resolve_java_receiver(
                    receiver,
                    owner.map(|symbol| symbol.name.as_str()),
                    &variables,
                    &fields,
                    &returns,
                    &type_params,
                    call.range.start_byte,
                ) else {
                    continue;
                };
                let type_symbols: Vec<_> = symbols
                    .symbols
                    .iter()
                    .filter(|symbol| {
                        symbol.name == ty
                            && matches!(
                                symbol.id.kind,
                                crate::resolution::SymbolKind::Struct
                                    | crate::resolution::SymbolKind::Enum
                                    | crate::resolution::SymbolKind::Union
                                    | crate::resolution::SymbolKind::Trait
                            )
                    })
                    .collect();
                let [type_symbol] = type_symbols.as_slice() else {
                    continue;
                };
                let call_args = call.scope_path.get(2..).unwrap_or(&[]);
                let argument_types: Vec<Option<String>> = call_args
                    .iter()
                    .map(|argument| {
                        infer_argument_type(argument, &variables, call.range.start_byte)
                    })
                    .collect();
                let members: Vec<_> = symbols
                    .symbols
                    .iter()
                    .filter(|symbol| {
                        if symbol.name != method
                            || symbol.id.path != type_symbol.id.path
                            || symbol.id.start_byte < type_symbol.id.start_byte
                            || symbol.id.end_byte > type_symbol.id.end_byte
                        {
                            return false;
                        }
                        let Some((signature_arity, parameters)) =
                            signatures.get(&(file.path.clone(), symbol.id.start_byte))
                        else {
                            return true;
                        };
                        if call
                            .scope_path
                            .get(1)
                            .is_some_and(|arity| arity != signature_arity)
                        {
                            return false;
                        }
                        if parameters.len() != call_args.len() {
                            return false;
                        }
                        parameters
                            .iter()
                            .zip(&argument_types)
                            .all(|(parameter, argument)| {
                                argument
                                    .as_deref()
                                    .is_none_or(|argument| normalize_type(argument) == *parameter)
                            })
                    })
                    .collect();
                let [member] = members.as_slice() else {
                    continue;
                };
                let Some(from) = enclosing_symbol(symbols, &file.path, call.range) else {
                    continue;
                };
                edges.push(ScopedSymbolEdge {
                    from: from.id.clone(),
                    to: member.id.clone(),
                    kind: ReferenceKind::MethodCall,
                    spelling: format!("{receiver}.{method}"),
                    range: call.range,
                });
            }
            for access in parse
                .facts
                .iter()
                .filter(|fact| fact.kind == SyntaxFactKind::FieldAccess)
            {
                let (Some(field), Some(receiver)) =
                    (access.name.as_deref(), access.scope_path.first())
                else {
                    continue;
                };
                let owner = enclosing_type_symbol(symbols, &file.path, access.range);
                let Some(ty) = resolve_java_receiver(
                    receiver,
                    owner.map(|symbol| symbol.name.as_str()),
                    &variables,
                    &fields,
                    &returns,
                    &type_params,
                    access.range.start_byte,
                ) else {
                    continue;
                };
                let type_symbols: Vec<_> = symbols
                    .symbols
                    .iter()
                    .filter(|symbol| {
                        symbol.name == ty
                            && matches!(
                                symbol.id.kind,
                                crate::resolution::SymbolKind::Struct
                                    | crate::resolution::SymbolKind::Enum
                                    | crate::resolution::SymbolKind::Union
                                    | crate::resolution::SymbolKind::Trait
                            )
                    })
                    .collect();
                let [type_symbol] = type_symbols.as_slice() else {
                    continue;
                };
                let members: Vec<_> = symbols
                    .symbols
                    .iter()
                    .filter(|symbol| {
                        symbol.name == field
                            && symbol.id.kind == crate::resolution::SymbolKind::Field
                            && symbol.id.path == type_symbol.id.path
                            && symbol.id.start_byte >= type_symbol.id.start_byte
                            && symbol.id.end_byte <= type_symbol.id.end_byte
                    })
                    .collect();
                let [member] = members.as_slice() else {
                    continue;
                };
                let Some(from) = enclosing_symbol(symbols, &file.path, access.range) else {
                    continue;
                };
                let write = access.scope_path.get(1).is_some_and(|mode| mode == "write");
                let operator = access
                    .scope_path
                    .get(2)
                    .map(String::as_str)
                    .unwrap_or("read");
                edges.push(ScopedSymbolEdge {
                    from: from.id.clone(),
                    to: member.id.clone(),
                    kind: if write {
                        ReferenceKind::FieldWrite
                    } else {
                        ReferenceKind::FieldAccess
                    },
                    spelling: if operator == "read" {
                        format!("{receiver}.{field}")
                    } else {
                        format!("{receiver}.{field} {operator}")
                    },
                    range: access.range,
                });
            }
        }
        edges
    }
}

fn enclosing_type_symbol<'a>(
    symbols: &'a SymbolCandidateReport,
    path: &Path,
    range: SourceRange,
) -> Option<&'a SymbolRecord> {
    symbols
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.id.path == path
                && matches!(
                    symbol.id.kind,
                    crate::resolution::SymbolKind::Struct
                        | crate::resolution::SymbolKind::Enum
                        | crate::resolution::SymbolKind::Union
                        | crate::resolution::SymbolKind::Trait
                )
                && symbol.id.start_byte <= range.start_byte
                && range.end_byte <= symbol.id.end_byte
        })
        .min_by_key(|symbol| symbol.id.end_byte.saturating_sub(symbol.id.start_byte))
}

fn normalize_type(text: &str) -> String {
    let text = text.trim();
    let text = text.split('<').next().unwrap_or(text);
    text.trim_end_matches(['[', ']', '?', ' ']).to_owned()
}

fn infer_argument_type(
    text: &str,
    variables: &[&crate::syntax::SyntaxFact],
    call_start: usize,
) -> Option<String> {
    let text = text.trim();
    if text.starts_with('"') || text.starts_with('\'') {
        return Some("String".to_owned());
    }
    if !text.is_empty() && text.chars().all(|ch| ch.is_ascii_digit() || ch == '-') {
        return Some("int".to_owned());
    }
    if let Some(rest) = text.strip_prefix("new ") {
        return Some(normalize_type(rest.split('(').next().unwrap_or(rest)));
    }
    if text
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.')
    {
        return variables
            .iter()
            .filter(|variable| {
                variable.name.as_deref() == Some(text) && variable.range.start_byte <= call_start
            })
            .max_by_key(|variable| variable.range.start_byte)
            .and_then(|variable| variable.scope_path.first())
            .map(|ty| normalize_type(ty));
    }
    None
}

fn resolve_receiver(
    receiver: &str,
    owner: Option<&str>,
    variables: &[&crate::syntax::SyntaxFact],
    fields: &BTreeMap<(String, String), String>,
    returns: &BTreeMap<String, Vec<String>>,
    call_start: usize,
) -> Option<String> {
    let mut segments = receiver.split('.').filter(|segment| !segment.is_empty());
    let first = segments.next()?;
    let mut current = receiver_segment(first, owner, variables, returns, call_start)?;
    for segment in segments {
        current = if let Some(name) = segment.strip_suffix("()") {
            let candidates = returns.get(name)?;
            let [return_type] = candidates.as_slice() else {
                return None;
            };
            return_type.clone()
        } else {
            fields.get(&(current.clone(), segment.to_owned()))?.clone()
        };
    }
    Some(current)
}

fn cast_type(expression: &str) -> Option<String> {
    let expression = expression.trim();
    if let Some(rest) = expression.strip_prefix('(')
        && let Some(end) = rest.find(')')
    {
        let ty = rest[..end].trim();
        if !ty.is_empty() {
            return Some(normalize_type(ty));
        }
    }
    if let Some((_, ty)) = expression.rsplit_once(" as ") {
        let ty = ty.trim();
        if !ty.is_empty() {
            return Some(normalize_type(ty));
        }
    }
    None
}

fn resolve_java_receiver(
    receiver: &str,
    owner: Option<&str>,
    variables: &[&crate::syntax::SyntaxFact],
    fields: &BTreeMap<(String, String), String>,
    returns: &BTreeMap<String, Vec<String>>,
    type_params: &BTreeMap<String, Vec<String>>,
    call_start: usize,
) -> Option<String> {
    let receiver = strip_outer_parens(receiver.trim());
    if let Some((left, right)) = split_conditional(receiver) {
        let left = resolve_java_receiver(
            left,
            owner,
            variables,
            fields,
            returns,
            type_params,
            call_start,
        )?;
        let right = resolve_java_receiver(
            right,
            owner,
            variables,
            fields,
            returns,
            type_params,
            call_start,
        )?;
        return (normalize_type(&left) == normalize_type(&right)).then_some(left);
    }
    let mut segments = receiver.split('.').filter(|segment| !segment.is_empty());
    let first = segments.next()?;
    let mut current = if let Some(cast) = cast_type(first) {
        cast
    } else if let Some((name, _)) = first.split_once('(')
        && first.ends_with(')')
    {
        java_method_return("", name, returns)?
    } else if first == "this" {
        owner?.to_owned()
    } else if first == "super" {
        return None;
    } else {
        let name = first.split('[').next().unwrap_or(first);
        variables
            .iter()
            .filter(|variable| {
                variable.name.as_deref() == Some(name) && variable.range.start_byte <= call_start
            })
            .max_by_key(|variable| variable.range.start_byte)?
            .scope_path
            .first()?
            .to_owned()
    };
    for segment in segments {
        if let Some((name, _)) = segment.split_once('(')
            && segment.ends_with(')')
        {
            current = java_method_return(&current, name, returns)?;
            continue;
        }
        let (base, arguments) = split_type(&current);
        let raw = fields.get(&(base.clone(), segment.to_owned()))?;
        current = substitute_type(raw, type_params.get(&base), &arguments);
    }
    Some(normalize_type(&current))
}

fn strip_outer_parens(expression: &str) -> &str {
    let mut expression = expression.trim();
    loop {
        let Some(inner) = expression
            .strip_prefix('(')
            .and_then(|value| value.strip_suffix(')'))
        else {
            return expression;
        };
        if balanced(inner) {
            expression = inner.trim();
        } else {
            return expression;
        }
    }
}

fn balanced(expression: &str) -> bool {
    let mut depth = 0i32;
    for ch in expression.chars() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
}

fn split_conditional(expression: &str) -> Option<(&str, &str)> {
    let mut depth = 0i32;
    let mut question = None;
    for (index, ch) in expression.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '?' if depth == 0 => question = Some(index),
            ':' if depth == 0 => {
                let start = question?;
                return Some((&expression[start + 1..index], &expression[index + 1..]));
            }
            _ => {}
        }
    }
    None
}

fn java_method_return(
    receiver: &str,
    name: &str,
    returns: &BTreeMap<String, Vec<String>>,
) -> Option<String> {
    let (base, arguments) = split_type(receiver);
    let builtin = match (base.as_str(), name) {
        ("List" | "ArrayList" | "Collection" | "Iterable" | "Stream", "get" | "findFirst") => {
            arguments.first().cloned()
        }
        ("List" | "ArrayList" | "Collection" | "Iterable", "iterator") => arguments
            .first()
            .map(|argument| format!("Iterator<{argument}>")),
        ("Iterator" | "Stream", "next") => arguments.first().cloned(),
        ("Optional", "get" | "orElse") => arguments.first().cloned(),
        ("Map" | "HashMap", "get") => arguments.get(1).cloned(),
        ("Supplier", "get") => arguments.first().cloned(),
        ("Function" | "UnaryOperator", "apply") => arguments.get(1).cloned(),
        ("BiFunction", "apply") => arguments.get(2).cloned(),
        _ => None,
    };
    if let Some(builtin) = builtin {
        return Some(builtin);
    }
    let candidates = returns.get(name)?;
    let [return_type] = candidates.as_slice() else {
        return None;
    };
    Some(return_type.clone())
}

fn split_type(ty: &str) -> (String, Vec<String>) {
    let ty = ty.trim();
    let Some(open) = ty.find('<') else {
        return (normalize_type(ty), Vec::new());
    };
    let base = normalize_type(&ty[..open]);
    let args = ty[open + 1..]
        .trim_end_matches('>')
        .split(',')
        .map(str::trim)
        .filter(|arg| !arg.is_empty())
        .map(normalize_type)
        .collect();
    (base, args)
}

fn substitute_type(raw: &str, params: Option<&Vec<String>>, args: &[String]) -> String {
    let Some(params) = params else {
        return raw.to_owned();
    };
    let mut result = raw.to_owned();
    for (position, param) in params.iter().enumerate() {
        if let Some(arg) = args.get(position) {
            result = result.replace(param, arg);
        }
    }
    result
}

fn receiver_segment(
    segment: &str,
    owner: Option<&str>,
    variables: &[&crate::syntax::SyntaxFact],
    returns: &BTreeMap<String, Vec<String>>,
    call_start: usize,
) -> Option<String> {
    if let Some(name) = segment.strip_suffix("()") {
        let candidates = returns.get(name)?;
        let [return_type] = candidates.as_slice() else {
            return None;
        };
        return Some(return_type.clone());
    }
    if let Some(cast) = cast_type(segment) {
        return Some(cast);
    }
    if segment == "this" {
        return owner.map(str::to_owned);
    }
    if segment == "super" {
        return None;
    }
    let segment = segment.split('[').next().unwrap_or(segment);
    variables
        .iter()
        .filter(|variable| {
            variable.name.as_deref() == Some(segment) && variable.range.start_byte <= call_start
        })
        .max_by_key(|variable| variable.range.start_byte)?
        .scope_path
        .first()
        .map(|ty| normalize_type(ty))
}

/// TypeScript receiver table: typed variables/fields to member lookup.
#[derive(Clone, Copy, Debug, Default)]
pub struct TypeScriptReceiverResolver;

impl SymbolResolver for TypeScriptReceiverResolver {
    fn language(&self) -> LanguageId {
        LanguageId::TypeScript
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        symbols: &SymbolCandidateReport,
        _imports: &[ImportEdge],
    ) -> Vec<ScopedSymbolEdge> {
        resolve_typed_receivers(files, symbols, LanguageId::TypeScript)
    }
}

fn resolve_typed_receivers(
    files: &[SourceFileResult],
    symbols: &SymbolCandidateReport,
    language: LanguageId,
) -> Vec<ScopedSymbolEdge> {
    let mut edges = Vec::new();
    for file in files {
        if file.language != Some(language) {
            continue;
        }
        let Some(parse) = &file.parse else {
            continue;
        };
        let variables: Vec<&crate::syntax::SyntaxFact> = parse
            .facts
            .iter()
            .filter(|fact| {
                fact.kind == SyntaxFactKind::Variable
                    && fact.name.is_some()
                    && !fact.scope_path.is_empty()
            })
            .collect();
        let mut fields: BTreeMap<(String, String), String> = BTreeMap::new();
        for variable in &variables {
            let Some(owner) = enclosing_type_symbol(symbols, &file.path, variable.range) else {
                continue;
            };
            if let (Some(name), Some(ty)) = (variable.name.as_deref(), variable.scope_path.first())
            {
                fields.insert((owner.name.clone(), name.to_owned()), normalize_type(ty));
            }
        }
        let mut returns: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut arities: BTreeMap<(std::path::PathBuf, usize), (String, Vec<String>)> =
            BTreeMap::new();
        for signature in parse
            .facts
            .iter()
            .filter(|fact| fact.kind == SyntaxFactKind::MethodSignature)
        {
            if let (Some(name), Some(return_type)) =
                (signature.name.as_deref(), signature.scope_path.first())
            {
                returns
                    .entry(name.to_owned())
                    .or_default()
                    .push(normalize_type(return_type));
                arities.insert(
                    (file.path.clone(), signature.range.start_byte),
                    (
                        signature.scope_path.get(1).cloned().unwrap_or_default(),
                        signature
                            .scope_path
                            .iter()
                            .skip(2)
                            .map(|ty| normalize_type(ty))
                            .collect(),
                    ),
                );
            }
        }
        for call in parse
            .facts
            .iter()
            .filter(|fact| fact.kind == SyntaxFactKind::ReceiverCall)
        {
            let (Some(method), Some(receiver)) = (call.name.as_deref(), call.scope_path.first())
            else {
                continue;
            };
            let owner = enclosing_type_symbol(symbols, &file.path, call.range);
            let Some(ty) = resolve_receiver(
                receiver,
                owner.map(|symbol| symbol.name.as_str()),
                &variables,
                &fields,
                &returns,
                call.range.start_byte,
            ) else {
                continue;
            };
            let type_symbols: Vec<_> = symbols
                .symbols
                .iter()
                .filter(|symbol| {
                    symbol.name == ty
                        && matches!(
                            symbol.id.kind,
                            crate::resolution::SymbolKind::Struct
                                | crate::resolution::SymbolKind::Trait
                        )
                })
                .collect();
            let [type_symbol] = type_symbols.as_slice() else {
                continue;
            };
            let call_args = call.scope_path.get(2..).unwrap_or(&[]);
            let argument_types: Vec<Option<String>> = call_args
                .iter()
                .map(|argument| infer_argument_type(argument, &variables, call.range.start_byte))
                .collect();
            let members: Vec<_> = symbols
                .symbols
                .iter()
                .filter(|symbol| {
                    if symbol.name != method
                        || symbol.id.path != type_symbol.id.path
                        || symbol.id.start_byte < type_symbol.id.start_byte
                        || symbol.id.end_byte > type_symbol.id.end_byte
                    {
                        return false;
                    }
                    let Some((arity, parameters)) =
                        arities.get(&(file.path.clone(), symbol.id.start_byte))
                    else {
                        return true;
                    };
                    if call.scope_path.get(1).is_some_and(|value| value != arity) {
                        return false;
                    }
                    parameters
                        .iter()
                        .zip(&argument_types)
                        .all(|(parameter, argument)| {
                            argument
                                .as_deref()
                                .is_none_or(|argument| normalize_type(argument) == *parameter)
                        })
                })
                .collect();
            let [member] = members.as_slice() else {
                continue;
            };
            let Some(from) = enclosing_symbol(symbols, &file.path, call.range) else {
                continue;
            };
            edges.push(ScopedSymbolEdge {
                from: from.id.clone(),
                to: member.id.clone(),
                kind: ReferenceKind::MethodCall,
                spelling: format!("{receiver}.{method}"),
                range: call.range,
            });
        }
        for access in parse
            .facts
            .iter()
            .filter(|fact| fact.kind == SyntaxFactKind::FieldAccess)
        {
            let (Some(field), Some(receiver)) = (access.name.as_deref(), access.scope_path.first())
            else {
                continue;
            };
            let owner = enclosing_type_symbol(symbols, &file.path, access.range);
            let Some(ty) = resolve_receiver(
                receiver,
                owner.map(|symbol| symbol.name.as_str()),
                &variables,
                &fields,
                &returns,
                access.range.start_byte,
            ) else {
                continue;
            };
            let type_symbols: Vec<_> = symbols
                .symbols
                .iter()
                .filter(|symbol| {
                    symbol.name == ty
                        && matches!(
                            symbol.id.kind,
                            crate::resolution::SymbolKind::Struct
                                | crate::resolution::SymbolKind::Trait
                        )
                })
                .collect();
            let [type_symbol] = type_symbols.as_slice() else {
                continue;
            };
            let members: Vec<_> = symbols
                .symbols
                .iter()
                .filter(|symbol| {
                    symbol.name == field
                        && symbol.id.kind == crate::resolution::SymbolKind::Field
                        && symbol.id.path == type_symbol.id.path
                        && symbol.id.start_byte >= type_symbol.id.start_byte
                        && symbol.id.end_byte <= type_symbol.id.end_byte
                })
                .collect();
            let [member] = members.as_slice() else {
                continue;
            };
            let Some(from) = enclosing_symbol(symbols, &file.path, access.range) else {
                continue;
            };
            let write = access.scope_path.get(1).is_some_and(|mode| mode == "write");
            let operator = access
                .scope_path
                .get(2)
                .map(String::as_str)
                .unwrap_or("read");
            edges.push(ScopedSymbolEdge {
                from: from.id.clone(),
                to: member.id.clone(),
                kind: if write {
                    ReferenceKind::FieldWrite
                } else {
                    ReferenceKind::FieldAccess
                },
                spelling: if operator == "read" {
                    format!("{receiver}.{field}")
                } else {
                    format!("{receiver}.{field} {operator}")
                },
                range: access.range,
            });
        }
    }
    edges
}

/// C# receiver table: typed variables/fields to member lookup.
#[derive(Clone, Copy, Debug, Default)]
pub struct CSharpReceiverResolver;

impl SymbolResolver for CSharpReceiverResolver {
    fn language(&self) -> LanguageId {
        LanguageId::CSharp
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        symbols: &SymbolCandidateReport,
        _imports: &[ImportEdge],
    ) -> Vec<ScopedSymbolEdge> {
        resolve_typed_receivers(files, symbols, LanguageId::CSharp)
    }
}

/// Kotlin receiver table: typed properties/parameters to member lookup.
#[derive(Clone, Copy, Debug, Default)]
pub struct KotlinReceiverResolver;

impl SymbolResolver for KotlinReceiverResolver {
    fn language(&self) -> LanguageId {
        LanguageId::Kotlin
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        symbols: &SymbolCandidateReport,
        _imports: &[ImportEdge],
    ) -> Vec<ScopedSymbolEdge> {
        resolve_typed_receivers(files, symbols, LanguageId::Kotlin)
    }
}

/// Scala receiver table: typed vals/parameters to member lookup.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScalaReceiverResolver;

impl SymbolResolver for ScalaReceiverResolver {
    fn language(&self) -> LanguageId {
        LanguageId::Scala
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        symbols: &SymbolCandidateReport,
        _imports: &[ImportEdge],
    ) -> Vec<ScopedSymbolEdge> {
        resolve_typed_receivers(files, symbols, LanguageId::Scala)
    }
}

/// Go receiver table: struct fields and receiver methods.
#[derive(Clone, Copy, Debug, Default)]
pub struct GoReceiverResolver;

impl SymbolResolver for GoReceiverResolver {
    fn language(&self) -> LanguageId {
        LanguageId::Go
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        symbols: &SymbolCandidateReport,
        _imports: &[ImportEdge],
    ) -> Vec<ScopedSymbolEdge> {
        let mut edges = Vec::new();
        for file in files {
            if file.language != Some(LanguageId::Go) {
                continue;
            }
            let Some(parse) = &file.parse else {
                continue;
            };
            let variables: Vec<_> = parse
                .facts
                .iter()
                .filter(|fact| {
                    fact.kind == SyntaxFactKind::Variable
                        && fact.name.is_some()
                        && !fact.scope_path.is_empty()
                })
                .collect();
            let mut fields: BTreeMap<(String, String), ()> = BTreeMap::new();
            for symbol in symbols
                .symbols
                .iter()
                .filter(|symbol| symbol.id.kind == crate::resolution::SymbolKind::Field)
            {
                for ty in symbols.symbols.iter().filter(|ty| {
                    matches!(
                        ty.id.kind,
                        crate::resolution::SymbolKind::Struct
                            | crate::resolution::SymbolKind::Enum
                            | crate::resolution::SymbolKind::Union
                    ) && ty.id.path == symbol.id.path
                        && symbol.id.start_byte >= ty.id.start_byte
                        && symbol.id.end_byte <= ty.id.end_byte
                }) {
                    fields.insert((ty.name.clone(), symbol.name.clone()), ());
                }
            }
            let mut methods: BTreeMap<(String, String), &SymbolRecord> = BTreeMap::new();
            for signature in parse
                .facts
                .iter()
                .filter(|fact| fact.kind == SyntaxFactKind::MethodSignature)
            {
                let (Some(name), Some(owner)) =
                    (signature.name.as_deref(), signature.scope_path.last())
                else {
                    continue;
                };
                if let Some(method) = symbols.symbols.iter().find(|symbol| {
                    symbol.name == name
                        && symbol.id.path == file.path
                        && symbol.id.start_byte == signature.range.start_byte
                }) {
                    methods.insert((normalize_type(owner), name.to_owned()), method);
                }
            }
            for call in parse
                .facts
                .iter()
                .filter(|fact| fact.kind == SyntaxFactKind::ReceiverCall)
            {
                let (Some(method), Some(receiver)) =
                    (call.name.as_deref(), call.scope_path.first())
                else {
                    continue;
                };
                let Some(ty) = variables
                    .iter()
                    .filter(|variable| {
                        variable.name.as_deref() == Some(receiver)
                            && variable.range.start_byte <= call.range.start_byte
                    })
                    .max_by_key(|variable| variable.range.start_byte)
                    .and_then(|variable| variable.scope_path.first())
                else {
                    continue;
                };
                let Some(target) = methods.get(&(normalize_type(ty), method.to_owned())) else {
                    continue;
                };
                let Some(from) = enclosing_symbol(symbols, &file.path, call.range) else {
                    continue;
                };
                edges.push(ScopedSymbolEdge {
                    from: from.id.clone(),
                    to: target.id.clone(),
                    kind: ReferenceKind::MethodCall,
                    spelling: format!("{receiver}.{method}"),
                    range: call.range,
                });
            }
            for access in parse
                .facts
                .iter()
                .filter(|fact| fact.kind == SyntaxFactKind::FieldAccess)
            {
                let (Some(field), Some(receiver)) =
                    (access.name.as_deref(), access.scope_path.first())
                else {
                    continue;
                };
                let Some(ty) = variables
                    .iter()
                    .filter(|variable| {
                        variable.name.as_deref() == Some(receiver)
                            && variable.range.start_byte <= access.range.start_byte
                    })
                    .max_by_key(|variable| variable.range.start_byte)
                    .and_then(|variable| variable.scope_path.first())
                else {
                    continue;
                };
                let key = (normalize_type(ty), field.to_owned());
                if !fields.contains_key(&key) {
                    continue;
                }
                let target = symbols.symbols.iter().find(|symbol| {
                    symbol.name == field
                        && symbol.id.kind == crate::resolution::SymbolKind::Field
                        && symbol.id.path == file.path
                });
                let (Some(target), Some(from)) =
                    (target, enclosing_symbol(symbols, &file.path, access.range))
                else {
                    continue;
                };
                edges.push(ScopedSymbolEdge {
                    from: from.id.clone(),
                    to: target.id.clone(),
                    kind: ReferenceKind::FieldAccess,
                    spelling: format!("{receiver}.{field}"),
                    range: access.range,
                });
            }
        }
        edges
    }
}

/// C header-scope resolver.
#[derive(Clone, Copy, Debug, Default)]
pub struct CIncludeSymbolResolver;

impl SymbolResolver for CIncludeSymbolResolver {
    fn language(&self) -> LanguageId {
        LanguageId::C
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        symbols: &SymbolCandidateReport,
        imports: &[ImportEdge],
    ) -> Vec<ScopedSymbolEdge> {
        resolve_file_imports_for(
            files,
            symbols,
            imports,
            &[LanguageId::C, LanguageId::CHeader],
        )
    }
}

/// C++ header-scope resolver.
#[derive(Clone, Copy, Debug, Default)]
pub struct CppSymbolResolver;

impl SymbolResolver for CppSymbolResolver {
    fn language(&self) -> LanguageId {
        LanguageId::Cpp
    }

    fn resolve(
        &self,
        files: &[SourceFileResult],
        symbols: &SymbolCandidateReport,
        imports: &[ImportEdge],
    ) -> Vec<ScopedSymbolEdge> {
        resolve_file_imports_for(
            files,
            symbols,
            imports,
            &[LanguageId::Cpp, LanguageId::CHeader],
        )
    }
}

fn resolve_file_imports(
    files: &[SourceFileResult],
    symbols: &SymbolCandidateReport,
    imports: &[ImportEdge],
    language: LanguageId,
) -> Vec<ScopedSymbolEdge> {
    resolve_file_imports_for(files, symbols, imports, &[language])
}

fn resolve_file_imports_for(
    files: &[SourceFileResult],
    symbols: &SymbolCandidateReport,
    imports: &[ImportEdge],
    languages: &[LanguageId],
) -> Vec<ScopedSymbolEdge> {
    let mut edges = Vec::new();
    for file in files {
        if !file
            .language
            .is_some_and(|language| languages.contains(&language))
        {
            continue;
        }
        let mut aliases: BTreeMap<&str, Vec<&SymbolRecord>> = BTreeMap::new();
        for import in imports.iter().filter(|import| import.from == file.path) {
            for symbol in symbols
                .symbols
                .iter()
                .filter(|symbol| symbol.id.path == import.to)
            {
                aliases
                    .entry(symbol.name.as_str())
                    .or_default()
                    .push(symbol);
            }
        }
        let Some(parse) = &file.parse else {
            continue;
        };
        for fact in &parse.facts {
            let Some(name) = fact.name.as_deref() else {
                continue;
            };
            let Some(candidates) = aliases.get(name) else {
                continue;
            };
            let [symbol] = candidates.as_slice() else {
                continue;
            };
            let Some(from) = enclosing_symbol(symbols, &file.path, fact.range) else {
                continue;
            };
            let kind = match fact.kind {
                SyntaxFactKind::Call => ReferenceKind::FunctionCall,
                SyntaxFactKind::MethodCall => ReferenceKind::MethodCall,
                SyntaxFactKind::TypeReference => ReferenceKind::TypeReference,
                _ => continue,
            };
            edges.push(ScopedSymbolEdge {
                from: from.id.clone(),
                to: symbol.id.clone(),
                kind,
                spelling: name.to_owned(),
                range: fact.range,
            });
        }
    }
    edges
}

fn resolve_qualified(
    files: &[SourceFileResult],
    symbols: &SymbolCandidateReport,
    imports: &[ImportEdge],
    language: LanguageId,
) -> Vec<ScopedSymbolEdge> {
    let mut edges = Vec::new();
    for file in files {
        if file.language != Some(language) {
            continue;
        }
        let mut aliases = Vec::new();
        for import in imports.iter().filter(|import| import.from == file.path) {
            let Some(name) = qualified_import_name(&import.spelling) else {
                continue;
            };
            if let Some(symbol) = symbols
                .symbols
                .iter()
                .find(|symbol| symbol.id.path == import.to && symbol.name == name)
            {
                aliases.push((name, symbol));
            }
        }
        let Some(parse) = &file.parse else {
            continue;
        };
        for fact in &parse.facts {
            let Some(name) = fact.name.as_deref() else {
                continue;
            };
            let Some((_, symbol)) = aliases.iter().find(|(local, _)| *local == name) else {
                continue;
            };
            let Some(from) = enclosing_symbol(symbols, &file.path, fact.range) else {
                continue;
            };
            let kind = match fact.kind {
                SyntaxFactKind::TypeReference => ReferenceKind::TypeReference,
                SyntaxFactKind::Call => ReferenceKind::FunctionCall,
                SyntaxFactKind::MethodCall => ReferenceKind::MethodCall,
                _ => continue,
            };
            edges.push(ScopedSymbolEdge {
                from: from.id.clone(),
                to: symbol.id.clone(),
                kind,
                spelling: name.to_owned(),
                range: fact.range,
            });
        }
    }
    edges
}

fn qualified_import_name(text: &str) -> Option<&str> {
    let text = text.trim();
    let text = text
        .strip_prefix("import ")
        .or_else(|| text.strip_prefix("use "))?;
    let text = text.split(" as ").next().unwrap_or(text);
    let text = text.trim_end_matches(';').trim();
    if text.ends_with('*') || text.ends_with('_') || text.contains('{') {
        return None;
    }
    text.rsplit(['.', '\\']).next()
}

fn named_imports(text: &str) -> Vec<(String, String)> {
    let Some(open) = text.find('{') else {
        return Vec::new();
    };
    let Some(close) = text.get(open..).and_then(|rest| rest.find('}')) else {
        return Vec::new();
    };
    let inner = &text[open + 1..open + close];
    inner
        .split(',')
        .filter_map(|entry| {
            let entry = entry.trim();
            if entry.is_empty() {
                return None;
            }
            let mut parts = entry.split(" as ");
            let target = parts.next()?.trim().to_owned();
            let local = parts.next().unwrap_or(&target).trim().to_owned();
            Some((target, local))
        })
        .collect()
}

fn enclosing_symbol<'a>(
    symbols: &'a SymbolCandidateReport,
    path: &Path,
    range: SourceRange,
) -> Option<&'a SymbolRecord> {
    symbols
        .symbols
        .iter()
        .filter(|symbol| {
            symbol.id.path == path
                && symbol.id.start_byte <= range.start_byte
                && range.end_byte <= symbol.id.end_byte
        })
        .min_by_key(|symbol| symbol.id.end_byte.saturating_sub(symbol.id.start_byte))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::WorkspaceId;
    use crate::index::{SourceFileResult, SourceFileStatus};
    use crate::resolution::SymbolKind;
    use crate::syntax::{ParseResult, ParseStatus, SyntaxFact};

    fn symbol(
        workspace: WorkspaceId,
        path: &str,
        start: usize,
        end: usize,
        name: &str,
    ) -> SymbolRecord {
        SymbolRecord {
            id: SymbolId {
                workspace_id: workspace,
                path: PathBuf::from(path),
                start_byte: start,
                end_byte: end,
                kind: SymbolKind::Struct,
            },
            name: name.to_owned(),
        }
    }

    fn file(workspace: WorkspaceId, path: &str, fact: SyntaxFact) -> SourceFileResult {
        file_lang(workspace, path, LanguageId::Java, fact)
    }

    fn file_lang(
        workspace: WorkspaceId,
        path: &str,
        language: LanguageId,
        fact: SyntaxFact,
    ) -> SourceFileResult {
        SourceFileResult {
            path: PathBuf::from(path),
            language: Some(language),
            status: SourceFileStatus::Parsed,
            parse: Some(ParseResult {
                workspace_id: workspace,
                relative_path: PathBuf::from(path),
                language,
                provider_id: "test".to_owned(),
                provider_version: "1".to_owned(),
                schema_version: 1,
                status: ParseStatus::Complete,
                limit_reason: None,
                facts: vec![fact],
                imports: Vec::new(),
                relations: Vec::new(),
                modules: Vec::new(),
                diagnostics: Vec::new(),
            }),
        }
    }

    #[test]
    fn java_import_binds_a_type_reference_to_the_imported_symbol() {
        let workspace = WorkspaceId::new();
        let foo = symbol(workspace, "com/example/Foo.java", 0, 10, "Foo");
        let main = symbol(workspace, "src/Main.java", 0, 100, "Main");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![foo.clone(), main.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let files = vec![file(
            workspace,
            "src/Main.java",
            SyntaxFact {
                kind: SyntaxFactKind::TypeReference,
                name: Some("Foo".to_owned()),
                scope_path: Vec::new(),
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 20,
                    end_byte: 23,
                },
            },
        )];
        let imports = vec![ImportEdge {
            from: PathBuf::from("src/Main.java"),
            to: PathBuf::from("com/example/Foo.java"),
            spelling: "import com.example.Foo;".to_owned(),
            range: SourceRange {
                start_byte: 0,
                end_byte: 10,
            },
        }];

        let edges = JavaSymbolResolver.resolve(&files, &symbols, &imports);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, main.id);
        assert_eq!(edges[0].to, foo.id);
        assert_eq!(edges[0].kind, ReferenceKind::TypeReference);
    }
    fn call_fact(name: &str, kind: SyntaxFactKind) -> SyntaxFact {
        SyntaxFact {
            kind,
            name: Some(name.to_owned()),
            scope_path: Vec::new(),
            is_module_scope: false,
            visibility: None,
            range: SourceRange {
                start_byte: 20,
                end_byte: 24,
            },
        }
    }

    #[test]
    fn go_package_symbols_bind_calls() {
        let workspace = WorkspaceId::new();
        let helper = symbol(workspace, "pkg/util.go", 0, 10, "Helper");
        let main = symbol(workspace, "src/main.go", 0, 100, "main");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![helper.clone(), main.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let files = vec![file_lang(
            workspace,
            "src/main.go",
            LanguageId::Go,
            call_fact("Helper", SyntaxFactKind::Call),
        )];
        let imports = vec![ImportEdge {
            from: PathBuf::from("src/main.go"),
            to: PathBuf::from("pkg/util.go"),
            spelling: r#"import "example.com/mod/pkg""#.to_owned(),
            range: SourceRange {
                start_byte: 0,
                end_byte: 8,
            },
        }];

        let edges = GoSymbolResolver.resolve(&files, &symbols, &imports);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, helper.id);
        assert_eq!(edges[0].from, main.id);
    }

    #[test]
    fn csharp_using_symbols_bind_type_references() {
        let workspace = WorkspaceId::new();
        let widget = symbol(workspace, "src/Widget.cs", 0, 10, "Widget");
        let program = symbol(workspace, "src/Program.cs", 0, 100, "Program");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![widget.clone(), program.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let files = vec![file_lang(
            workspace,
            "src/Program.cs",
            LanguageId::CSharp,
            call_fact("Widget", SyntaxFactKind::TypeReference),
        )];
        let imports = vec![ImportEdge {
            from: PathBuf::from("src/Program.cs"),
            to: PathBuf::from("src/Widget.cs"),
            spelling: "using App.Widget;".to_owned(),
            range: SourceRange {
                start_byte: 0,
                end_byte: 8,
            },
        }];

        let edges = CSharpSymbolResolver.resolve(&files, &symbols, &imports);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, widget.id);
    }

    #[test]
    fn ruby_required_symbols_bind_calls() {
        let workspace = WorkspaceId::new();
        let helper = symbol(workspace, "lib/foo.rb", 0, 10, "helper");
        let run = symbol(workspace, "src/app.rb", 0, 100, "run");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![helper.clone(), run.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let files = vec![file_lang(
            workspace,
            "src/app.rb",
            LanguageId::Ruby,
            call_fact("helper", SyntaxFactKind::Call),
        )];
        let imports = vec![ImportEdge {
            from: PathBuf::from("src/app.rb"),
            to: PathBuf::from("lib/foo.rb"),
            spelling: r#"require_relative "../lib/foo""#.to_owned(),
            range: SourceRange {
                start_byte: 0,
                end_byte: 8,
            },
        }];

        let edges = RubySymbolResolver.resolve(&files, &symbols, &imports);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, helper.id);
    }
    #[test]
    fn c_header_symbols_bind_type_references() {
        let workspace = WorkspaceId::new();
        let widget = symbol(workspace, "include/widget.h", 0, 10, "Widget");
        let main = symbol(workspace, "src/main.c", 0, 100, "main");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![widget.clone(), main.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let files = vec![file_lang(
            workspace,
            "src/main.c",
            LanguageId::C,
            call_fact("Widget", SyntaxFactKind::TypeReference),
        )];
        let imports = vec![ImportEdge {
            from: PathBuf::from("src/main.c"),
            to: PathBuf::from("include/widget.h"),
            spelling: r#"#include "widget.h""#.to_owned(),
            range: SourceRange {
                start_byte: 0,
                end_byte: 8,
            },
        }];

        let edges = CIncludeSymbolResolver.resolve(&files, &symbols, &imports);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, widget.id);
    }
    #[test]
    fn java_receiver_type_table_binds_a_member_call() {
        let workspace = WorkspaceId::new();
        let ty = symbol(workspace, "src/Foo.java", 0, 200, "Foo");
        let helper = symbol(workspace, "src/Foo.java", 50, 65, "helper");
        let run = symbol(workspace, "src/Foo.java", 60, 100, "run");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![ty.clone(), helper.clone(), run.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let mut source = file_lang(
            workspace,
            "src/Foo.java",
            LanguageId::Java,
            SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("foo".to_owned()),
                scope_path: vec!["Foo".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 10,
                    end_byte: 20,
                },
            },
        );
        source
            .parse
            .as_mut()
            .expect("parse")
            .facts
            .push(SyntaxFact {
                kind: SyntaxFactKind::ReceiverCall,
                name: Some("helper".to_owned()),
                scope_path: vec!["foo".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 70,
                    end_byte: 80,
                },
            });

        let edges = JavaReceiverResolver.resolve(&[source], &symbols, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, run.id);
        assert_eq!(edges[0].to, helper.id);
        assert_eq!(edges[0].kind, ReferenceKind::MethodCall);
    }
    #[test]
    fn java_receiver_resolves_this_field_chain() {
        let workspace = WorkspaceId::new();
        let holder = symbol(workspace, "src/Holder.java", 0, 300, "Holder");
        let foo = symbol(workspace, "src/Holder.java", 100, 200, "Foo");
        let helper = symbol(workspace, "src/Holder.java", 120, 140, "helper");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![holder.clone(), foo.clone(), helper.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let mut source = file_lang(
            workspace,
            "src/Holder.java",
            LanguageId::Java,
            SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("foo".to_owned()),
                scope_path: vec!["Foo".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 10,
                    end_byte: 20,
                },
            },
        );
        source
            .parse
            .as_mut()
            .expect("parse")
            .facts
            .push(SyntaxFact {
                kind: SyntaxFactKind::ReceiverCall,
                name: Some("helper".to_owned()),
                scope_path: vec!["this.foo".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 250,
                    end_byte: 260,
                },
            });

        let edges = JavaReceiverResolver.resolve(&[source], &symbols, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, holder.id);
        assert_eq!(edges[0].to, helper.id);
    }

    #[test]
    fn java_receiver_normalizes_generic_base_types() {
        let workspace = WorkspaceId::new();
        let list = symbol(workspace, "src/List.java", 0, 200, "List");
        let helper = symbol(workspace, "src/List.java", 50, 80, "helper");
        let owner = symbol(workspace, "src/Main.java", 0, 300, "Main");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![list.clone(), helper.clone(), owner.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let mut source = file_lang(
            workspace,
            "src/Main.java",
            LanguageId::Java,
            SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("items".to_owned()),
                scope_path: vec!["List<Foo>".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 10,
                    end_byte: 20,
                },
            },
        );
        source
            .parse
            .as_mut()
            .expect("parse")
            .facts
            .push(SyntaxFact {
                kind: SyntaxFactKind::ReceiverCall,
                name: Some("helper".to_owned()),
                scope_path: vec!["items".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 50,
                    end_byte: 60,
                },
            });

        let edges = JavaReceiverResolver.resolve(&[source], &symbols, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, helper.id);
    }

    #[test]
    fn java_receiver_uses_call_arity_for_overloads() {
        let workspace = WorkspaceId::new();
        let ty = symbol(workspace, "src/Foo.java", 0, 200, "Foo");
        let zero = symbol(workspace, "src/Foo.java", 50, 60, "helper");
        let one = symbol(workspace, "src/Foo.java", 70, 80, "helper");
        let run = symbol(workspace, "src/Foo.java", 60, 100, "run");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![ty.clone(), zero.clone(), one.clone(), run.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let mut source = file_lang(
            workspace,
            "src/Foo.java",
            LanguageId::Java,
            SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("foo".to_owned()),
                scope_path: vec!["Foo".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 10,
                    end_byte: 20,
                },
            },
        );
        {
            let parse = source.parse.as_mut().expect("parse");
            for (range, arity) in [((50, 60), "0"), ((70, 80), "1")] {
                parse.facts.push(SyntaxFact {
                    kind: SyntaxFactKind::MethodSignature,
                    name: Some("helper".to_owned()),
                    scope_path: vec!["Bar".to_owned(), arity.to_owned()],
                    is_module_scope: false,
                    visibility: None,
                    range: SourceRange {
                        start_byte: range.0,
                        end_byte: range.1,
                    },
                });
            }
            parse.facts.push(SyntaxFact {
                kind: SyntaxFactKind::ReceiverCall,
                name: Some("helper".to_owned()),
                scope_path: vec!["foo".to_owned(), "1".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 60,
                    end_byte: 70,
                },
            });
        }

        let edges = JavaReceiverResolver.resolve(&[source], &symbols, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, one.id);
        assert_eq!(edges[0].from, run.id);
    }
    #[test]
    fn java_receiver_binds_field_access_to_a_field_symbol() {
        let workspace = WorkspaceId::new();
        let ty = symbol(workspace, "src/Foo.java", 0, 200, "Foo");
        let field = SymbolRecord {
            id: SymbolId {
                workspace_id: workspace,
                path: PathBuf::from("src/Foo.java"),
                start_byte: 20,
                end_byte: 30,
                kind: crate::resolution::SymbolKind::Field,
            },
            name: "bar".to_owned(),
        };
        let run = symbol(workspace, "src/Foo.java", 50, 100, "run");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![ty.clone(), field.clone(), run.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let mut source = file_lang(
            workspace,
            "src/Foo.java",
            LanguageId::Java,
            SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("foo".to_owned()),
                scope_path: vec!["Foo".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 10,
                    end_byte: 20,
                },
            },
        );
        source
            .parse
            .as_mut()
            .expect("parse")
            .facts
            .push(SyntaxFact {
                kind: SyntaxFactKind::FieldAccess,
                name: Some("bar".to_owned()),
                scope_path: vec!["foo".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 60,
                    end_byte: 70,
                },
            });

        let edges = JavaReceiverResolver.resolve(&[source], &symbols, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, run.id);
        assert_eq!(edges[0].to, field.id);
        assert_eq!(edges[0].kind, ReferenceKind::FieldAccess);
    }
    #[test]
    fn java_receiver_matches_overload_by_argument_type() {
        let workspace = WorkspaceId::new();
        let ty = symbol(workspace, "src/Foo.java", 0, 200, "Foo");
        let int_helper = symbol(workspace, "src/Foo.java", 50, 60, "helper");
        let string_helper = symbol(workspace, "src/Foo.java", 70, 80, "helper");
        let run = symbol(workspace, "src/Foo.java", 60, 100, "run");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![ty.clone(), int_helper, string_helper.clone(), run.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let mut source = file_lang(
            workspace,
            "src/Foo.java",
            LanguageId::Java,
            SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("foo".to_owned()),
                scope_path: vec!["Foo".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 10,
                    end_byte: 20,
                },
            },
        );
        {
            let parse = source.parse.as_mut().expect("parse");
            parse.facts.push(SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("s".to_owned()),
                scope_path: vec!["String".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 20,
                    end_byte: 30,
                },
            });
            for (start, end, parameter) in [(50, 60, "int"), (70, 80, "String")] {
                parse.facts.push(SyntaxFact {
                    kind: SyntaxFactKind::MethodSignature,
                    name: Some("helper".to_owned()),
                    scope_path: vec!["void".to_owned(), "1".to_owned(), parameter.to_owned()],
                    is_module_scope: false,
                    visibility: None,
                    range: SourceRange {
                        start_byte: start,
                        end_byte: end,
                    },
                });
            }
            parse.facts.push(SyntaxFact {
                kind: SyntaxFactKind::ReceiverCall,
                name: Some("helper".to_owned()),
                scope_path: vec!["foo".to_owned(), "1".to_owned(), "s".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 60,
                    end_byte: 70,
                },
            });
        }

        let edges = JavaReceiverResolver.resolve(&[source], &symbols, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, string_helper.id);
        assert_eq!(edges[0].from, run.id);
    }
    #[test]
    fn java_receiver_marks_field_write_edges() {
        let workspace = WorkspaceId::new();
        let ty = symbol(workspace, "src/Foo.java", 0, 200, "Foo");
        let field = SymbolRecord {
            id: SymbolId {
                workspace_id: workspace,
                path: PathBuf::from("src/Foo.java"),
                start_byte: 20,
                end_byte: 30,
                kind: crate::resolution::SymbolKind::Field,
            },
            name: "bar".to_owned(),
        };
        let run = symbol(workspace, "src/Foo.java", 50, 100, "run");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![ty, field, run],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let mut source = file_lang(
            workspace,
            "src/Foo.java",
            LanguageId::Java,
            SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("foo".to_owned()),
                scope_path: vec!["Foo".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 10,
                    end_byte: 20,
                },
            },
        );
        source
            .parse
            .as_mut()
            .expect("parse")
            .facts
            .push(SyntaxFact {
                kind: SyntaxFactKind::FieldAccess,
                name: Some("bar".to_owned()),
                scope_path: vec!["foo".to_owned(), "write".to_owned(), "+=".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 60,
                    end_byte: 70,
                },
            });

        let edges = JavaReceiverResolver.resolve(&[source], &symbols, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].kind, ReferenceKind::FieldWrite);
        assert!(edges[0].spelling.contains("+="));
    }

    #[test]
    fn java_receiver_resolves_field_on_a_method_return() {
        let workspace = WorkspaceId::new();
        let owner = symbol(workspace, "src/Foo.java", 0, 300, "Foo");
        let bar = symbol(workspace, "src/Foo.java", 100, 200, "Bar");
        let field = SymbolRecord {
            id: SymbolId {
                workspace_id: workspace,
                path: PathBuf::from("src/Foo.java"),
                start_byte: 120,
                end_byte: 130,
                kind: crate::resolution::SymbolKind::Field,
            },
            name: "field".to_owned(),
        };
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![owner.clone(), bar, field.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let mut source = file_lang(
            workspace,
            "src/Foo.java",
            LanguageId::Java,
            SyntaxFact {
                kind: SyntaxFactKind::MethodSignature,
                name: Some("getFoo".to_owned()),
                scope_path: vec!["Bar".to_owned(), "0".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 40,
                    end_byte: 50,
                },
            },
        );
        source
            .parse
            .as_mut()
            .expect("parse")
            .facts
            .push(SyntaxFact {
                kind: SyntaxFactKind::FieldAccess,
                name: Some("field".to_owned()),
                scope_path: vec!["getFoo()".to_owned(), "read".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 210,
                    end_byte: 220,
                },
            });

        let edges = JavaReceiverResolver.resolve(&[source], &symbols, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, field.id);
        assert_eq!(edges[0].from, owner.id);
    }
    #[test]
    fn java_receiver_resolves_array_element_fields() {
        let workspace = WorkspaceId::new();
        let ty = symbol(workspace, "src/Foo.java", 0, 200, "Foo");
        let field = SymbolRecord {
            id: SymbolId {
                workspace_id: workspace,
                path: PathBuf::from("src/Foo.java"),
                start_byte: 20,
                end_byte: 30,
                kind: crate::resolution::SymbolKind::Field,
            },
            name: "bar".to_owned(),
        };
        let run = symbol(workspace, "src/Foo.java", 50, 100, "run");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![ty, field.clone(), run],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let mut source = file_lang(
            workspace,
            "src/Foo.java",
            LanguageId::Java,
            SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("items".to_owned()),
                scope_path: vec!["Foo[]".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 10,
                    end_byte: 20,
                },
            },
        );
        source
            .parse
            .as_mut()
            .expect("parse")
            .facts
            .push(SyntaxFact {
                kind: SyntaxFactKind::FieldAccess,
                name: Some("bar".to_owned()),
                scope_path: vec!["items[i]".to_owned(), "read".to_owned(), "read".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 60,
                    end_byte: 70,
                },
            });

        let edges = JavaReceiverResolver.resolve(&[source], &symbols, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, field.id);
    }
    #[test]
    fn typescript_receiver_table_binds_a_member_call() {
        let workspace = WorkspaceId::new();
        let ty = symbol(workspace, "src/Foo.ts", 0, 200, "Bar");
        let helper = symbol(workspace, "src/Foo.ts", 50, 65, "helper");
        let run = symbol(workspace, "src/Foo.ts", 60, 100, "run");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![ty, helper.clone(), run.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let mut source = file_lang(
            workspace,
            "src/Foo.ts",
            LanguageId::TypeScript,
            SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("foo".to_owned()),
                scope_path: vec!["Bar".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 10,
                    end_byte: 20,
                },
            },
        );
        source
            .parse
            .as_mut()
            .expect("parse")
            .facts
            .push(SyntaxFact {
                kind: SyntaxFactKind::ReceiverCall,
                name: Some("helper".to_owned()),
                scope_path: vec!["foo".to_owned(), "0".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 70,
                    end_byte: 80,
                },
            });

        let edges = TypeScriptReceiverResolver.resolve(&[source], &symbols, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, run.id);
        assert_eq!(edges[0].to, helper.id);
    }
    #[test]
    fn kotlin_receiver_table_binds_a_member_call() {
        let workspace = WorkspaceId::new();
        let ty = symbol(workspace, "src/Foo.kt", 0, 200, "Bar");
        let helper = symbol(workspace, "src/Foo.kt", 50, 65, "helper");
        let run = symbol(workspace, "src/Foo.kt", 60, 100, "run");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![ty, helper.clone(), run.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let mut source = file_lang(
            workspace,
            "src/Foo.kt",
            LanguageId::Kotlin,
            SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("foo".to_owned()),
                scope_path: vec!["Bar".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 10,
                    end_byte: 20,
                },
            },
        );
        source
            .parse
            .as_mut()
            .expect("parse")
            .facts
            .push(SyntaxFact {
                kind: SyntaxFactKind::ReceiverCall,
                name: Some("helper".to_owned()),
                scope_path: vec!["foo".to_owned(), "0".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 70,
                    end_byte: 80,
                },
            });

        let edges = KotlinReceiverResolver.resolve(&[source], &symbols, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, run.id);
        assert_eq!(edges[0].to, helper.id);
    }
    #[test]
    fn scala_receiver_table_binds_a_member_call() {
        let workspace = WorkspaceId::new();
        let ty = symbol(workspace, "src/Foo.scala", 0, 200, "Bar");
        let helper = symbol(workspace, "src/Foo.scala", 50, 65, "helper");
        let run = symbol(workspace, "src/Foo.scala", 60, 100, "run");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![ty, helper.clone(), run.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let mut source = file_lang(
            workspace,
            "src/Foo.scala",
            LanguageId::Scala,
            SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("foo".to_owned()),
                scope_path: vec!["Bar".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 10,
                    end_byte: 20,
                },
            },
        );
        source
            .parse
            .as_mut()
            .expect("parse")
            .facts
            .push(SyntaxFact {
                kind: SyntaxFactKind::ReceiverCall,
                name: Some("helper".to_owned()),
                scope_path: vec!["foo".to_owned(), "0".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 70,
                    end_byte: 80,
                },
            });

        let edges = ScalaReceiverResolver.resolve(&[source], &symbols, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, run.id);
        assert_eq!(edges[0].to, helper.id);
    }
    #[test]
    fn go_receiver_table_binds_receiver_methods() {
        let workspace = WorkspaceId::new();
        let ty = symbol(workspace, "src/main.go", 0, 100, "Bar");
        let helper = symbol(workspace, "src/main.go", 110, 150, "Helper");
        let run = symbol(workspace, "src/main.go", 150, 200, "run");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![ty, helper.clone(), run.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let mut source = file_lang(
            workspace,
            "src/main.go",
            LanguageId::Go,
            SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("y".to_owned()),
                scope_path: vec!["Bar".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 120,
                    end_byte: 130,
                },
            },
        );
        {
            let parse = source.parse.as_mut().expect("parse");
            parse.facts.push(SyntaxFact {
                kind: SyntaxFactKind::MethodSignature,
                name: Some("Helper".to_owned()),
                scope_path: vec![
                    "Baz".to_owned(),
                    "1".to_owned(),
                    "Qux".to_owned(),
                    "Bar".to_owned(),
                ],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 110,
                    end_byte: 150,
                },
            });
            parse.facts.push(SyntaxFact {
                kind: SyntaxFactKind::ReceiverCall,
                name: Some("Helper".to_owned()),
                scope_path: vec!["y".to_owned(), "1".to_owned(), "q".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 160,
                    end_byte: 170,
                },
            });
        }

        let edges = GoReceiverResolver.resolve(&[source], &symbols, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].from, run.id);
        assert_eq!(edges[0].to, helper.id);
    }
    #[test]
    fn receiver_resolves_cast_expressions() {
        let workspace = WorkspaceId::new();
        let ty = symbol(workspace, "src/Foo.java", 0, 200, "Foo");
        let field = SymbolRecord {
            id: SymbolId {
                workspace_id: workspace,
                path: PathBuf::from("src/Foo.java"),
                start_byte: 20,
                end_byte: 30,
                kind: crate::resolution::SymbolKind::Field,
            },
            name: "bar".to_owned(),
        };
        let run = symbol(workspace, "src/Foo.java", 50, 100, "run");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![ty, field.clone(), run.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let mut source = file_lang(
            workspace,
            "src/Foo.java",
            LanguageId::Java,
            SyntaxFact {
                kind: SyntaxFactKind::FieldAccess,
                name: Some("bar".to_owned()),
                scope_path: vec!["(Foo) x".to_owned(), "read".to_owned(), "read".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 60,
                    end_byte: 70,
                },
            },
        );
        source.parse.as_mut().expect("parse").facts.clear();
        source
            .parse
            .as_mut()
            .expect("parse")
            .facts
            .push(SyntaxFact {
                kind: SyntaxFactKind::FieldAccess,
                name: Some("bar".to_owned()),
                scope_path: vec!["(Foo) x".to_owned(), "read".to_owned(), "read".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 60,
                    end_byte: 70,
                },
            });

        let edges = JavaReceiverResolver.resolve(&[source], &symbols, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, field.id);
        assert_eq!(edges[0].from, run.id);
    }
    #[test]
    fn java_generic_field_substitution_reaches_the_member() {
        let workspace = WorkspaceId::new();
        let box_ty = symbol(workspace, "src/Main.java", 0, 100, "Box");
        let foo_ty = symbol(workspace, "src/Main.java", 100, 200, "Foo");
        let value = SymbolRecord {
            id: SymbolId {
                workspace_id: workspace,
                path: PathBuf::from("src/Main.java"),
                start_byte: 10,
                end_byte: 20,
                kind: crate::resolution::SymbolKind::Field,
            },
            name: "value".to_owned(),
        };
        let next = SymbolRecord {
            id: SymbolId {
                workspace_id: workspace,
                path: PathBuf::from("src/Main.java"),
                start_byte: 110,
                end_byte: 120,
                kind: crate::resolution::SymbolKind::Field,
            },
            name: "next".to_owned(),
        };
        let run = symbol(workspace, "src/Main.java", 200, 300, "run");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![box_ty, foo_ty, value, next.clone(), run.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let mut source = file_lang(
            workspace,
            "src/Main.java",
            LanguageId::Java,
            SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("box".to_owned()),
                scope_path: vec!["Box<Foo>".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 30,
                    end_byte: 40,
                },
            },
        );
        {
            let parse = source.parse.as_mut().expect("parse");
            parse.facts.push(SyntaxFact {
                kind: SyntaxFactKind::TypeParameter,
                name: Some("T".to_owned()),
                scope_path: vec!["Box".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 1,
                    end_byte: 2,
                },
            });
            parse.facts.push(SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("value".to_owned()),
                scope_path: vec!["T".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 10,
                    end_byte: 20,
                },
            });
            parse.facts.push(SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("next".to_owned()),
                scope_path: vec!["Bar".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 110,
                    end_byte: 120,
                },
            });
            parse.facts.push(SyntaxFact {
                kind: SyntaxFactKind::FieldAccess,
                name: Some("next".to_owned()),
                scope_path: vec!["box.value".to_owned(), "read".to_owned(), "read".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 210,
                    end_byte: 220,
                },
            });
        }

        let edges = JavaReceiverResolver.resolve(&[source], &symbols, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, next.id);
        assert_eq!(edges[0].from, run.id);
    }
    #[test]
    fn java_collection_generic_return_reaches_element_member() {
        let workspace = WorkspaceId::new();
        let foo = symbol(workspace, "src/Main.java", 100, 200, "Foo");
        let next = SymbolRecord {
            id: SymbolId {
                workspace_id: workspace,
                path: PathBuf::from("src/Main.java"),
                start_byte: 110,
                end_byte: 120,
                kind: crate::resolution::SymbolKind::Field,
            },
            name: "next".to_owned(),
        };
        let run = symbol(workspace, "src/Main.java", 200, 300, "run");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![foo, next.clone(), run.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let mut source = file_lang(
            workspace,
            "src/Main.java",
            LanguageId::Java,
            SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("list".to_owned()),
                scope_path: vec!["List<Foo>".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 30,
                    end_byte: 40,
                },
            },
        );
        {
            let parse = source.parse.as_mut().expect("parse");
            parse.facts.push(SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("next".to_owned()),
                scope_path: vec!["Bar".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 110,
                    end_byte: 120,
                },
            });
            parse.facts.push(SyntaxFact {
                kind: SyntaxFactKind::FieldAccess,
                name: Some("next".to_owned()),
                scope_path: vec![
                    "list.get()".to_owned(),
                    "read".to_owned(),
                    "read".to_owned(),
                ],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 210,
                    end_byte: 220,
                },
            });
        }

        let edges = JavaReceiverResolver.resolve(&[source], &symbols, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, next.id);
        assert_eq!(edges[0].from, run.id);
    }
    #[test]
    fn java_conditional_receiver_resolves_equal_branch_types() {
        let workspace = WorkspaceId::new();
        let foo = symbol(workspace, "src/Main.java", 100, 200, "Foo");
        let next = SymbolRecord {
            id: SymbolId {
                workspace_id: workspace,
                path: PathBuf::from("src/Main.java"),
                start_byte: 110,
                end_byte: 120,
                kind: crate::resolution::SymbolKind::Field,
            },
            name: "next".to_owned(),
        };
        let run = symbol(workspace, "src/Main.java", 200, 300, "run");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![foo, next.clone(), run.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let mut source = file_lang(
            workspace,
            "src/Main.java",
            LanguageId::Java,
            SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("a".to_owned()),
                scope_path: vec!["Foo".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 150,
                    end_byte: 160,
                },
            },
        );
        {
            let parse = source.parse.as_mut().expect("parse");
            parse.facts.push(SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("b".to_owned()),
                scope_path: vec!["Foo".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 160,
                    end_byte: 170,
                },
            });
            parse.facts.push(SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("next".to_owned()),
                scope_path: vec!["Bar".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 110,
                    end_byte: 120,
                },
            });
            parse.facts.push(SyntaxFact {
                kind: SyntaxFactKind::FieldAccess,
                name: Some("next".to_owned()),
                scope_path: vec![
                    "(flag ? a : b)".to_owned(),
                    "read".to_owned(),
                    "read".to_owned(),
                ],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 210,
                    end_byte: 220,
                },
            });
        }

        let edges = JavaReceiverResolver.resolve(&[source], &symbols, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, next.id);
    }

    #[test]
    fn java_functional_interface_return_substitutes() {
        let workspace = WorkspaceId::new();
        let bar = symbol(workspace, "src/Main.java", 100, 200, "Bar");
        let next = SymbolRecord {
            id: SymbolId {
                workspace_id: workspace,
                path: PathBuf::from("src/Main.java"),
                start_byte: 110,
                end_byte: 120,
                kind: crate::resolution::SymbolKind::Field,
            },
            name: "next".to_owned(),
        };
        let run = symbol(workspace, "src/Main.java", 200, 300, "run");
        let symbols = SymbolCandidateReport {
            workspace_id: workspace,
            symbols: vec![bar, next.clone(), run.clone()],
            references: Vec::new(),
            unique_candidate_references: 0,
            ambiguous_references: 0,
            unresolved_references: 0,
            unsupported_references: 0,
        };
        let mut source = file_lang(
            workspace,
            "src/Main.java",
            LanguageId::Java,
            SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("f".to_owned()),
                scope_path: vec!["Function<Foo,Bar>".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 10,
                    end_byte: 20,
                },
            },
        );
        {
            let parse = source.parse.as_mut().expect("parse");
            parse.facts.push(SyntaxFact {
                kind: SyntaxFactKind::Variable,
                name: Some("next".to_owned()),
                scope_path: vec!["Baz".to_owned()],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 110,
                    end_byte: 120,
                },
            });
            parse.facts.push(SyntaxFact {
                kind: SyntaxFactKind::FieldAccess,
                name: Some("next".to_owned()),
                scope_path: vec![
                    "f.apply(x)".to_owned(),
                    "read".to_owned(),
                    "read".to_owned(),
                ],
                is_module_scope: false,
                visibility: None,
                range: SourceRange {
                    start_byte: 210,
                    end_byte: 220,
                },
            });
        }

        let edges = JavaReceiverResolver.resolve(&[source], &symbols, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to, next.id);
    }
}
