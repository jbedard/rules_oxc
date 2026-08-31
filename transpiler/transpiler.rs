use oxc::allocator::Allocator;
use oxc::ast::ast::{
    ArrowFunctionExpression, AwaitExpression, ForOfStatement, Function, Program, Statement,
    VariableDeclaration, VariableDeclarationKind,
};
use oxc::ast_visit::{Visit, walk};
use oxc::codegen::{Codegen, CodegenOptions, CommentOptions};
use oxc::diagnostics::{GraphicalReportHandler, GraphicalTheme, NamedSource, OxcDiagnostic};
use oxc::isolated_declarations::{
    IsolatedDeclarations, IsolatedDeclarationsOptions as OxcIsolatedDeclarationsOptions,
};
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::{GetSpan, SourceType, Span};
use oxc::syntax::{module_record::ModuleRecord, scope::ScopeFlags};
use oxc::transformer::{
    CompilerAssumptions, DecoratorOptions, EnvOptions, HelperLoaderOptions, JsxOptions, JsxRuntime,
    Module, RewriteExtensionsMode, TransformOptions, Transformer, TypeScriptOptions,
};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

#[derive(Default)]
struct Options {
    emit_js: bool,
    emit_dts: bool,
    source_maps: bool,
    inline_source_maps: bool,
    declaration_maps: bool,
    source_root: Option<String>,
    // Used as the base for map sources when source_root changes how consumers resolve them.
    source_root_dir: Option<PathBuf>,
    remove_comments: bool,
    jsx: JsxRuntime,
    rewrite_extensions: bool,
    env: Option<EnvOptions>,
    helpers_module: Option<String>,
    module: Module,
    jsx_import_source: Option<String>,
    jsx_pragma: Option<String>,
    jsx_pragma_frag: Option<String>,
    use_define_for_class_fields: bool,
    strip_internal: bool,
    only_remove_type_imports: bool,
    experimental_decorators: bool,
    emit_decorator_metadata: bool,
    no_strict_null_checks: bool,
}

struct Entry {
    src: String,
    js_out: Option<String>,
    dts_out: Option<String>,
}

fn main() {
    if let Err(errors) = run(std::env::args().skip(1)) {
        for error in &errors {
            eprintln!("{error}");
        }
        std::process::exit(1);
    }
}

fn required_flag_value(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
    what: &str,
) -> Result<String, Vec<String>> {
    args.next()
        .ok_or_else(|| vec![format!("error: {flag} requires {what}")])
}

fn run(args: impl Iterator<Item = String>) -> Result<(), Vec<String>> {
    let Cli {
        options,
        cpus,
        manifest_path,
        positional,
    } = parse_args(args)?;
    let entries = load_entries(&options, manifest_path, positional)?;
    transpile_and_write_entries(&options, cpus, &entries)
}

const SUPPORTED_TARGETS: [&str; 14] = [
    "es6", "es2015", "es2016", "es2017", "es2018", "es2019", "es2020", "es2021", "es2022",
    "es2023", "es2024", "es2025", "es2026", "esnext",
];

struct Cli {
    options: Options,
    cpus: usize,
    manifest_path: Option<String>,
    positional: Vec<String>,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Cli, Vec<String>> {
    let mut cli = Cli {
        options: Options::default(),
        cpus: 1,
        manifest_path: None,
        positional: Vec::new(),
    };

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--emit-js" => cli.options.emit_js = true,
            "--emit-dts" => cli.options.emit_dts = true,
            "--source-maps" => cli.options.source_maps = true,
            "--inline-source-maps" => cli.options.inline_source_maps = true,
            "--declaration-maps" => cli.options.declaration_maps = true,
            "--source-root" => {
                cli.options.source_root = Some(required_flag_value(&mut args, &arg, "a value")?);
            }
            "--source-root-dir" => {
                cli.options.source_root_dir = Some(PathBuf::from(required_flag_value(
                    &mut args,
                    &arg,
                    "a directory path",
                )?));
            }
            "--remove-comments" => cli.options.remove_comments = true,
            "--cpus" => {
                let value = required_flag_value(&mut args, &arg, "a positive integer")?;
                cli.cpus = value
                    .parse()
                    .ok()
                    .filter(|cpus: &usize| *cpus > 0)
                    .ok_or_else(|| {
                        vec![format!(
                            "error: --cpus must be a positive integer, got \"{value}\""
                        )]
                    })?;
            }
            "--jsx" => {
                let value = required_flag_value(&mut args, &arg, "a value")?;
                cli.options.jsx = match value.as_str() {
                    "automatic" => JsxRuntime::Automatic,
                    "classic" => JsxRuntime::Classic,
                    _ => {
                        return Err(vec![format!(
                            "error: unsupported --jsx \"{value}\": expected \"automatic\" or \"classic\""
                        )]);
                    }
                };
            }
            "--jsx-import-source" => {
                cli.options.jsx_import_source =
                    Some(required_flag_value(&mut args, &arg, "a value")?);
            }
            "--jsx-pragma" => {
                cli.options.jsx_pragma = Some(required_flag_value(&mut args, &arg, "a value")?);
            }
            "--jsx-pragma-frag" => {
                cli.options.jsx_pragma_frag =
                    Some(required_flag_value(&mut args, &arg, "a value")?);
            }
            "--rewrite-extensions" => cli.options.rewrite_extensions = true,
            "--use-define-for-class-fields" => cli.options.use_define_for_class_fields = true,
            "--strip-internal" => cli.options.strip_internal = true,
            "--only-remove-type-imports" => cli.options.only_remove_type_imports = true,
            "--experimental-decorators" => cli.options.experimental_decorators = true,
            "--emit-decorator-metadata" => cli.options.emit_decorator_metadata = true,
            "--no-strict-null-checks" => cli.options.no_strict_null_checks = true,
            "--target" => {
                let target = required_flag_value(&mut args, &arg, "a value")?;
                // Repeats the Bazel rule's attr allowlist: only whole ES versions that oxc can
                // fully downlevel to. Rejects es5 (oxc cannot fully downlevel ES2015 syntax) and
                // engine/browserslist targets, which EnvOptions::from_target would accept.
                if !SUPPORTED_TARGETS.contains(&target.as_str()) {
                    return Err(vec![format!(
                        "error: unsupported --target \"{target}\": expected es6, es2015..es2026, or esnext"
                    )]);
                }
                cli.options.env =
                    Some(EnvOptions::from_target(&target).expect("allowlisted target"));
            }
            "--helpers-module" => {
                cli.options.helpers_module = Some(required_flag_value(&mut args, &arg, "a value")?);
            }
            "--module" => {
                let value = required_flag_value(&mut args, &arg, "a value")?;
                cli.options.module = match value.as_str() {
                    "preserve" => Module::Preserve,
                    "esm" => Module::Esm,
                    "commonjs" => Module::CommonJS,
                    _ => {
                        return Err(vec![format!(
                            "error: unsupported --module \"{value}\": expected \"preserve\", \"esm\", or \"commonjs\""
                        )]);
                    }
                };
            }
            "--manifest" => {
                cli.manifest_path = Some(required_flag_value(&mut args, &arg, "a file path")?);
            }
            _ if arg.starts_with("--") => {
                return Err(vec![format!("error: unknown flag \"{arg}\"")]);
            }
            _ => cli.positional.push(arg),
        }
    }

    if cli.options.jsx_import_source.is_some() && cli.options.jsx != JsxRuntime::Automatic {
        return Err(vec![
            "error: --jsx-import-source requires --jsx automatic".to_string(),
        ]);
    }

    if (cli.options.jsx_pragma.is_some() || cli.options.jsx_pragma_frag.is_some())
        && cli.options.jsx != JsxRuntime::Classic
    {
        return Err(vec![
            "error: --jsx-pragma and --jsx-pragma-frag require --jsx classic".to_string(),
        ]);
    }

    if cli.options.emit_decorator_metadata && !cli.options.experimental_decorators {
        return Err(vec![
            "error: --emit-decorator-metadata requires --experimental-decorators".to_string(),
        ]);
    }

    if !cli.options.emit_js && !cli.options.emit_dts {
        return Err(vec![
            "error: at least one of --emit-js or --emit-dts is required".to_string(),
        ]);
    }

    if cli.options.declaration_maps && !cli.options.emit_dts {
        return Err(vec![
            "error: --declaration-maps requires --emit-dts".to_string(),
        ]);
    }

    if cli.options.source_maps && cli.options.inline_source_maps {
        return Err(vec![
            "error: --source-maps and --inline-source-maps are mutually exclusive".to_string(),
        ]);
    }

    if cli.options.source_root.is_some()
        && !(cli.options.source_maps
            || cli.options.inline_source_maps
            || cli.options.declaration_maps)
    {
        return Err(vec![
            "error: --source-root requires --source-maps, --inline-source-maps, or --declaration-maps"
                .to_string(),
        ]);
    }

    if cli.options.source_root_dir.is_some() && cli.options.source_root.is_none() {
        return Err(vec![
            "error: --source-root-dir requires --source-root".to_string(),
        ]);
    }

    Ok(cli)
}

// Manifest entries contain a source followed by the enabled output paths. Empty paths skip output.
fn load_entries(
    options: &Options,
    manifest_path: Option<String>,
    positional: Vec<String>,
) -> Result<Vec<Entry>, Vec<String>> {
    let entry_width = 1 + options.emit_js as usize + options.emit_dts as usize;

    let lines: Vec<String> = if let Some(path) = manifest_path {
        let content = fs::read_to_string(&path)
            .map_err(|e| vec![format!("error: cannot read manifest {path}: {e}")])?;
        content.lines().map(str::to_string).collect()
    } else {
        positional
    };

    if !lines.len().is_multiple_of(entry_width) {
        return Err(vec![format!(
            "error: expected entries of {entry_width} lines (src followed by output paths), got {} lines",
            lines.len()
        )]);
    }

    let mut lines = lines.into_iter();
    let mut entries = Vec::new();
    while let Some(src) = lines.next() {
        let mut output = |emit: bool| {
            emit.then(|| lines.next().unwrap())
                .filter(|out| !out.is_empty())
        };
        let js_out = output(options.emit_js);
        let dts_out = output(options.emit_dts);
        entries.push(Entry {
            src,
            js_out,
            dts_out,
        });
    }
    Ok(entries)
}

// Keep processing after failures and return errors in manifest order.
fn transpile_and_write_entries(
    options: &Options,
    cpus: usize,
    entries: &[Entry],
) -> Result<(), Vec<String>> {
    let transform_options = build_transform_options(options);
    let next = AtomicUsize::new(0);
    let mut errors: Vec<(usize, Vec<String>)> = thread::scope(|scope| {
        let workers: Vec<_> = (0..cpus.min(entries.len()))
            .map(|_| {
                scope.spawn(|| {
                    let mut worker = Worker {
                        options,
                        transform_options: &transform_options,
                        allocator: Allocator::default(),
                    };
                    let mut errors = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(entry) = entries.get(i) else {
                            break errors;
                        };
                        let entry_errors = worker.transpile_and_write_entry(entry);
                        if !entry_errors.is_empty() {
                            errors.push((i, entry_errors));
                        }
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap())
            .collect()
    });
    errors.sort_by_key(|(i, _)| *i);

    let all_errors: Vec<String> = errors.into_iter().flat_map(|(_, e)| e).collect();
    if all_errors.is_empty() {
        Ok(())
    } else {
        Err(all_errors)
    }
}

// Records --source-root in a codegen map. A macro rather than a function: the map type is not
// re-exported by the oxc facade, so it is rebuilt from its parts through inference, not named.
macro_rules! with_source_root {
    ($map:expr, $options:expr) => {{
        let mut map = $map;
        if let Some(root) = &$options.source_root {
            if let Some(m) = map.take() {
                let mut parts = m.into_parts();
                parts.source_root = Some(std::borrow::Cow::Owned(root.clone()));
                map = Some(parts.into());
            }
        }
        map
    }};
}

// Per-thread transpile state: the arena is reset between files instead of reallocated.
struct Worker<'a> {
    options: &'a Options,
    transform_options: &'a TransformOptions,
    allocator: Allocator,
}

impl Worker<'_> {
    fn transpile_and_write_entry(&mut self, entry: &Entry) -> Vec<String> {
        // Declaration files have nothing to transpile and, matching tsc, are never emitted.
        if is_declaration_file(&entry.src) {
            return vec![format!(
                "error: declaration file \"{}\" produces no outputs and cannot be a transpile entry",
                entry.src
            )];
        }

        let content = match fs::read_to_string(&entry.src) {
            Ok(content) => content,
            Err(e) => return vec![format!("error: cannot read {}: {e}", entry.src)],
        };

        // sourceRoot makes consumers resolve sources relative to a different base.
        let map_source = |out: &Option<String>| {
            if let Some(root_dir) = &self.options.source_root_dir {
                return path_relative_to(Path::new(&entry.src), root_dir);
            }
            out.as_deref()
                .and_then(|out| Path::new(out).parent())
                .map(|out_dir| path_relative_to(Path::new(&entry.src), out_dir))
                .unwrap_or_else(|| PathBuf::from(&entry.src))
        };
        let js_map_source = map_source(&entry.js_out);
        let dts_map_source = map_source(&entry.dts_out);
        let outputs = match self.transpile_source(
            &entry.src,
            &content,
            entry.js_out.is_some(),
            entry.dts_out.is_some(),
            &js_map_source,
            &dts_map_source,
        ) {
            Ok(outputs) => outputs,
            Err(errors) => return errors,
        };

        let mut errors = Vec::new();
        if let (Some(js_out), Some(code)) = (&entry.js_out, outputs.js_code) {
            let map = outputs.js_map.map(|map| {
                if self.options.inline_source_maps {
                    SourceMapOutput::Inline(map)
                } else {
                    SourceMapOutput::File(map)
                }
            });
            if let Err(e) = write_output(js_out, &code, map.as_ref()) {
                errors.push(format!("error: cannot write {js_out}: {e}"));
            }
        }
        if let (Some(dts_out), Some(code)) = (&entry.dts_out, outputs.dts_code) {
            let map = outputs.dts_map.map(SourceMapOutput::File);
            if let Err(e) = write_output(dts_out, &code, map.as_ref()) {
                errors.push(format!("error: cannot write {dts_out}: {e}"));
            }
        }
        errors
    }

    // Emit declarations before the transformer mutates the shared AST.
    fn transpile_source(
        &mut self,
        filename: &str,
        source_text: &str,
        emit_js: bool,
        emit_dts: bool,
        js_map_source: &Path,
        dts_map_source: &Path,
    ) -> Result<Outputs, Vec<String>> {
        // Every source is parsed as TypeScript, including plain .js/.jsx: one pipeline for all inputs.
        let source_type = SourceType::from_path(filename)
            .unwrap_or_default()
            .with_typescript(true);

        self.allocator.reset();
        let allocator = &self.allocator;
        let mut parser_ret = Parser::new(allocator, source_text, source_type).parse();
        check_diagnostics(filename, source_text, parser_ret.diagnostics)?;

        let mut outputs = Outputs::default();

        if emit_dts {
            let decl_ret = IsolatedDeclarations::new(
                allocator,
                OxcIsolatedDeclarationsOptions {
                    strip_internal: self.options.strip_internal,
                },
            )
            .build(&parser_ret.program);
            check_diagnostics(filename, source_text, decl_ret.diagnostics)?;
            let codegen_ret = Codegen::new()
                .with_options(build_codegen_options(
                    self.options,
                    self.options
                        .declaration_maps
                        .then(|| dts_map_source.to_path_buf()),
                ))
                .build(&decl_ret.program);
            outputs.dts_code = Some(codegen_ret.code);
            outputs.dts_map =
                with_source_root!(codegen_ret.map, self.options).map(|m| m.to_json_string());
        }

        if emit_js {
            // Pre-evaluation prevents reverse mappings for string enum members.
            let semantic_ret = SemanticBuilder::new_compiler()
                .with_enum_eval(true)
                .with_excess_capacity(2.0)
                .build(&parser_ret.program);
            let semantic_diagnostics = semantic_ret.diagnostics;
            let scoping = semantic_ret.semantic.into_scoping();

            let transformer_ret =
                Transformer::new(allocator, Path::new(filename), self.transform_options)
                    .build_with_scoping(scoping, &mut parser_ret.program);
            let diagnostics = semantic_diagnostics
                .into_iter()
                .chain(transformer_ret.diagnostics);
            check_diagnostics(filename, source_text, diagnostics)?;

            if self.options.module.is_commonjs() {
                // The transformer may leave `export {}` after erasing type-only module syntax.
                parser_ret
                    .program
                    .body
                    .retain(|stmt| !is_empty_export(stmt));
                let diagnostics =
                    commonjs_diagnostics(&parser_ret.program, &parser_ret.module_record);
                check_diagnostics(filename, source_text, diagnostics)?;
            }

            let codegen_ret = Codegen::new()
                .with_options(build_codegen_options(
                    self.options,
                    (self.options.source_maps || self.options.inline_source_maps)
                        .then(|| js_map_source.to_path_buf()),
                ))
                .build(&parser_ret.program);

            outputs.js_code = Some(codegen_ret.code);
            let map = with_source_root!(codegen_ret.map, self.options);
            outputs.js_map = map.map(|map| {
                if self.options.inline_source_maps {
                    map.to_data_url()
                } else {
                    map.to_json_string()
                }
            });
        }

        Ok(outputs)
    }
}

enum SourceMapOutput {
    File(String),
    Inline(String),
}

fn write_output(path: &str, code: &str, map: Option<&SourceMapOutput>) -> std::io::Result<()> {
    let out = Path::new(path);
    let mut file = fs::File::create(out)?;
    file.write_all(code.as_bytes())?;
    if let Some(map) = map {
        if !code.ends_with('\n') {
            file.write_all(b"\n")?;
        }
        match map {
            SourceMapOutput::File(json) => {
                write!(
                    file,
                    "//# sourceMappingURL={}.map",
                    out.file_name().unwrap().to_string_lossy()
                )?;
                fs::write(format!("{path}.map"), json)?;
            }
            SourceMapOutput::Inline(url) => write!(file, "//# sourceMappingURL={url}")?,
        }
    }
    Ok(())
}

// Both paths are relative to the same root, or both are absolute.
fn path_relative_to(target: &Path, base_dir: &Path) -> PathBuf {
    let mut target_parts = target
        .components()
        .filter(|component| *component != Component::CurDir)
        .peekable();
    let mut base_parts = base_dir
        .components()
        .filter(|component| *component != Component::CurDir)
        .peekable();
    loop {
        match (target_parts.peek(), base_parts.peek()) {
            (Some(t), Some(b)) if t == b => {}
            _ => break,
        }
        target_parts.next();
        base_parts.next();
    }
    let mut relative: PathBuf = base_parts.map(|_| Component::ParentDir).collect();
    relative.extend(target_parts);
    relative
}

fn is_declaration_file(path: &str) -> bool {
    path.ends_with(".d.ts") || path.ends_with(".d.mts") || path.ends_with(".d.cts")
}

#[derive(Debug, Default)]
struct Outputs {
    js_code: Option<String>,
    js_map: Option<String>,
    dts_code: Option<String>,
    dts_map: Option<String>,
}

fn build_codegen_options(options: &Options, source_map_path: Option<PathBuf>) -> CodegenOptions {
    CodegenOptions {
        source_map_path,
        comments: if options.remove_comments {
            CommentOptions {
                normal: false,
                jsdoc: false,
                ..CommentOptions::default()
            }
        } else {
            CommentOptions::default()
        },
        ..Default::default()
    }
}

fn build_transform_options(options: &Options) -> TransformOptions {
    TransformOptions {
        assumptions: CompilerAssumptions {
            set_public_class_fields: !options.use_define_for_class_fields,
            ..Default::default()
        },
        typescript: {
            let mut typescript = TypeScriptOptions {
                remove_class_fields_without_initializer: !options.use_define_for_class_fields,
                only_remove_type_imports: options.only_remove_type_imports,
                // Like tsc's rewriteRelativeImportExtensions, but Babel semantics: any
                // slash-containing .ts/.tsx/.mts/.cts specifier is rewritten, and .tsx always
                // maps to .js (never .jsx).
                rewrite_import_extensions: options
                    .rewrite_extensions
                    .then_some(RewriteExtensionsMode::Rewrite),
                ..Default::default()
            };
            // The TypeScript transform must know the classic pragma so its import is not
            // stripped as type-only/unused.
            if let Some(pragma) = &options.jsx_pragma {
                typescript.jsx_pragma = pragma.clone().into();
            }
            if let Some(pragma_frag) = &options.jsx_pragma_frag {
                typescript.jsx_pragma_frag = pragma_frag.clone().into();
            }
            typescript
        },
        decorator: DecoratorOptions {
            legacy: options.experimental_decorators,
            emit_decorator_metadata: options.emit_decorator_metadata,
            strict_null_checks: !options.no_strict_null_checks,
        },
        jsx: JsxOptions {
            runtime: options.jsx,
            import_source: options.jsx_import_source.clone(),
            pragma: options.jsx_pragma.clone(),
            pragma_frag: options.jsx_pragma_frag.clone(),
            ..JsxOptions::default()
        },
        env: {
            let mut env = options.env.clone().unwrap_or_default();
            env.module = options.module;
            env
        },
        helper_loader: {
            let mut helper_loader = HelperLoaderOptions::default();
            if let Some(name) = &options.helpers_module {
                helper_loader.module_name = name.clone().into();
            }
            helper_loader
        },
        ..Default::default()
    }
}

// Finds module-level await (await expressions and `for await` outside any function), which is
// ESM-only syntax that oxc cannot rewrite for CommonJS.
struct TopLevelAwaitFinder {
    spans: Vec<Span>,
}

impl<'a> Visit<'a> for TopLevelAwaitFinder {
    fn visit_await_expression(&mut self, expr: &AwaitExpression<'a>) {
        self.spans.push(expr.span);
    }

    fn visit_for_of_statement(&mut self, stmt: &ForOfStatement<'a>) {
        if stmt.r#await {
            self.spans.push(stmt.span);
        } else {
            walk::walk_for_of_statement(self, stmt);
        }
    }

    fn visit_variable_declaration(&mut self, decl: &VariableDeclaration<'a>) {
        if decl.kind == VariableDeclarationKind::AwaitUsing {
            self.spans.push(decl.span);
        } else {
            walk::walk_variable_declaration(self, decl);
        }
    }

    fn visit_function(&mut self, _func: &Function<'a>, _flags: ScopeFlags) {}

    fn visit_arrow_function_expression(&mut self, _func: &ArrowFunctionExpression<'a>) {}
}

fn is_empty_export(stmt: &Statement) -> bool {
    matches!(stmt, Statement::ExportNamedDeclaration(decl) if decl.specifiers.is_empty())
}

// The transformer rewrites `export =`/`import x = require(...)` and erases type-only imports,
// but oxc has no ESM-to-CommonJS transform: any module syntax still present cannot be emitted
// as CommonJS. That includes `import.meta` (recorded in the module record) and top-level await
// (found by an AST walk).
fn commonjs_diagnostics(program: &Program, module_record: &ModuleRecord) -> Vec<OxcDiagnostic> {
    let mut await_finder = TopLevelAwaitFinder { spans: Vec::new() };
    await_finder.visit_program(program);
    program
        .body
        .iter()
        .filter(|stmt| stmt.is_module_declaration())
        .map(|stmt| {
            OxcDiagnostic::error(
                "ESM import/export syntax cannot be emitted as CommonJS: \
                 oxc only rewrites TypeScript `export =` and `import x = require(...)`",
            )
            .with_label(stmt.span())
        })
        .chain(module_record.import_metas.iter().map(|span| {
            OxcDiagnostic::error("import.meta cannot be emitted as CommonJS").with_label(*span)
        }))
        .chain(await_finder.spans.iter().map(|span| {
            OxcDiagnostic::error("top-level await cannot be emitted as CommonJS").with_label(*span)
        }))
        .collect()
}

fn check_diagnostics(
    filename: &str,
    source_text: &str,
    diagnostics: impl IntoIterator<Item = OxcDiagnostic>,
) -> Result<(), Vec<String>> {
    let mut diagnostics = diagnostics.into_iter().peekable();
    if diagnostics.peek().is_none() {
        return Ok(());
    }
    let handler = GraphicalReportHandler::new().with_theme(GraphicalTheme::none());
    // Arc-shared so each diagnostic does not clone the whole source text.
    let source = std::sync::Arc::new(NamedSource::new(filename, source_text.to_string()));
    Err(diagnostics
        .map(|diagnostic| {
            let diagnostic = diagnostic.with_source_code(source.clone());
            let mut s = String::new();
            handler.render_report(&mut s, diagnostic.as_ref()).unwrap();
            s
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let base = std::env::var_os("TEST_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = base.join(name);
        fs::create_dir_all(dir.join("out")).unwrap();
        dir
    }

    fn args<'a>(list: &'a [&'a str]) -> impl Iterator<Item = String> + 'a {
        list.iter().map(|s| s.to_string())
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    fn run_in(
        dir: &Path,
        flags: &[&str],
        paths: impl IntoIterator<Item = impl AsRef<Path>>,
    ) -> Result<(), Vec<String>> {
        let mut cli: Vec<String> = flags.iter().map(|s| s.to_string()).collect();
        cli.extend(
            paths
                .into_iter()
                .map(|p| dir.join(p).to_str().unwrap().to_string()),
        );
        run(cli.into_iter())
    }

    fn transpile(
        filename: &str,
        source_text: &str,
        options: &Options,
    ) -> Result<Outputs, Vec<String>> {
        Worker {
            options,
            transform_options: &build_transform_options(options),
            allocator: Allocator::default(),
        }
        .transpile_source(
            filename,
            source_text,
            options.emit_js,
            options.emit_dts,
            Path::new(filename),
            Path::new(filename),
        )
    }

    fn default_options() -> Options {
        Options {
            emit_js: true,
            emit_dts: true,
            ..Options::default()
        }
    }

    fn js_options() -> Options {
        Options {
            emit_dts: false,
            ..default_options()
        }
    }

    fn target_options(target: &str) -> Options {
        Options {
            env: Some(EnvOptions::from_target(target).unwrap()),
            ..js_options()
        }
    }

    fn commonjs_options() -> Options {
        Options {
            module: Module::CommonJS,
            ..js_options()
        }
    }

    fn esm_options() -> Options {
        Options {
            module: Module::Esm,
            ..js_options()
        }
    }

    fn transpile_js(filename: &str, source_text: &str, options: &Options) -> String {
        transpile(filename, source_text, options)
            .unwrap()
            .js_code
            .unwrap()
    }

    fn transpile_err(filename: &str, source_text: &str, options: &Options) -> String {
        transpile(filename, source_text, options)
            .unwrap_err()
            .concat()
    }

    fn transpile_single_err(filename: &str, source_text: &str, options: &Options) -> String {
        let errors = transpile(filename, source_text, options).unwrap_err();
        assert_eq!(errors.len(), 1, "errors: {errors:?}");
        errors.into_iter().next().unwrap()
    }

    #[test]
    fn target_downlevels_exponentiation() {
        let js = transpile_js(
            "a.ts",
            "export const x: number = 2 ** 10;\n",
            &target_options("es2015"),
        );
        assert!(js.contains("Math.pow(2, 10)"), "js: {js}");
    }

    #[test]
    fn target_downlevels_optional_chaining() {
        let js = transpile_js(
            "a.ts",
            "export function f(o: { a?: { b?: number } }): number { return o.a?.b ?? 0; }\n",
            &target_options("es2019"),
        );
        assert!(!js.contains("?."), "js: {js}");
        assert!(!js.contains("??"), "js: {js}");
    }

    #[test]
    fn target_downlevels_async_with_runtime_helper() {
        let js = transpile_js(
            "a.ts",
            "export async function f(): Promise<number> { return 1; }\n",
            &target_options("es2016"),
        );
        assert!(!js.contains("async function f"), "js: {js}");
        assert!(js.contains("@oxc-project/runtime"), "js: {js}");
    }

    #[test]
    fn no_target_keeps_modern_syntax() {
        let js = transpile_js("a.ts", "export const x: number = 2 ** 10;\n", &js_options());
        assert!(js.contains("2 ** 10"), "js: {js}");
    }

    #[test]
    fn run_reports_invalid_target() {
        for target in ["es1999", "es5", "chrome58", "node20", "es2015,chrome58"] {
            let err = run(args(&["--emit-js", "--target", target])).unwrap_err();
            assert!(
                err[0].starts_with(&format!("error: unsupported --target \"{target}\"")),
                "{err:?}"
            );
        }
    }

    #[test]
    fn run_accepts_supported_targets() {
        for target in SUPPORTED_TARGETS {
            let err = run(args(&["--target", target])).unwrap_err();
            assert_eq!(
                err,
                vec!["error: at least one of --emit-js or --emit-dts is required".to_string()],
                "target {target}"
            );
        }
    }

    #[test]
    fn run_requires_flag_values() {
        for (flag, value) in [
            ("--target", "a value"),
            ("--helpers-module", "a value"),
            ("--module", "a value"),
            ("--source-root", "a value"),
            ("--source-root-dir", "a directory path"),
            ("--jsx", "a value"),
            ("--jsx-import-source", "a value"),
            ("--jsx-pragma", "a value"),
            ("--jsx-pragma-frag", "a value"),
            ("--cpus", "a positive integer"),
            ("--manifest", "a file path"),
        ] {
            let err = run(args(&["--emit-js", flag])).unwrap_err();
            assert_eq!(err, vec![format!("error: {flag} requires {value}")]);
        }
    }

    fn helpers_options(helpers_module: Option<&str>) -> Options {
        Options {
            helpers_module: helpers_module.map(str::to_string),
            ..target_options("es2016")
        }
    }

    #[test]
    fn helpers_imported_from_default_module() {
        let js = transpile_js(
            "a.ts",
            "export async function f(): Promise<number> { return 1; }\n",
            &helpers_options(None),
        );
        assert!(
            js.contains("\"@oxc-project/runtime/helpers/asyncToGenerator\""),
            "js: {js}"
        );
    }

    #[test]
    fn helpers_module_redirects_helper_imports() {
        let js = transpile_js(
            "a.ts",
            "export async function f(): Promise<number> { return 1; }\n",
            &helpers_options(Some("custom-helpers")),
        );
        assert!(
            js.contains("\"custom-helpers/helpers/asyncToGenerator\""),
            "js: {js}"
        );
        assert!(!js.contains("@oxc-project/runtime"), "js: {js}");
    }

    #[test]
    fn helpers_module_leaves_helperless_output_unchanged() {
        let options = Options {
            helpers_module: Some("custom-helpers".to_string()),
            ..js_options()
        };
        let js = transpile_js("a.ts", "export const x: number = 1;\n", &options);
        assert!(!js.contains("custom-helpers"), "js: {js}");
    }

    #[test]
    fn esm_keeps_esm_syntax() {
        let js = transpile_js(
            "a.ts",
            "import { helper } from \"./b.js\";\nexport const x: number = helper;\n",
            &esm_options(),
        );
        assert!(js.contains("import { helper } from \"./b.js\""), "js: {js}");
        assert!(js.contains("export const x = helper"), "js: {js}");
        assert!(!js.contains("use strict"), "js: {js}");
    }

    #[test]
    fn esm_rejects_export_assignment() {
        let err = transpile_err(
            "a.ts",
            "const x: number = 1;\nexport = x;\n",
            &esm_options(),
        );
        assert!(err.contains("Export assignment cannot be used"), "{err}");
    }

    #[test]
    fn esm_rejects_import_equals() {
        let err = transpile_err(
            "a.ts",
            "import path = require(\"node:path\");\nexport const s: string = path.sep;\n",
            &esm_options(),
        );
        assert!(err.contains("Import assignment cannot be used"), "{err}");
    }

    #[test]
    fn commonjs_transforms_export_assignment() {
        let js = transpile_js(
            "a.cts",
            "const answer: number = 42;\nexport = { answer };\n",
            &commonjs_options(),
        );
        assert!(js.contains("\"use strict\""), "js: {js}");
        assert!(js.contains("module.exports = { answer }"), "js: {js}");
    }

    #[test]
    fn commonjs_transforms_import_equals() {
        let js = transpile_js(
            "a.cts",
            "import path = require(\"node:path\");\nexport = path.sep;\n",
            &commonjs_options(),
        );
        assert!(js.contains("require(\"node:path\")"), "js: {js}");
        assert!(!js.contains("import "), "js: {js}");
    }

    #[test]
    fn commonjs_erases_type_only_imports() {
        let js = transpile_js(
            "a.cts",
            "import type { Dirent } from \"node:fs\";\nexport = (d: Dirent): string => d.name;\n",
            &commonjs_options(),
        );
        assert!(!js.contains("node:fs"), "js: {js}");
        assert!(!js.contains("export {}"), "js: {js}");
    }

    #[test]
    fn commonjs_drops_empty_export() {
        let js = transpile_js(
            "a.cts",
            "const x: number = 1;\nexport = x;\nexport {};\n",
            &commonjs_options(),
        );
        assert!(!js.contains("export {}"), "js: {js}");
        assert!(js.contains("module.exports = x"), "js: {js}");
    }

    // `export {} from "..."` loads its source for side effects: it is a separate AST variant not
    // matched by the empty-export drop, and is rejected like any other ESM syntax.
    #[test]
    fn commonjs_rejects_sourced_empty_export() {
        let err = transpile_single_err(
            "a.cts",
            "const x: number = 1;\nexport = x;\nexport {} from \"./setup.cjs\";\n",
            &commonjs_options(),
        );
        assert!(err.contains("cannot be emitted as CommonJS"), "{err}");
    }

    #[test]
    fn commonjs_rejects_esm_import() {
        let err = transpile_single_err(
            "a.cts",
            "import { x } from \"./b.cjs\";\nexport = x;\n",
            &commonjs_options(),
        );
        assert!(err.contains("cannot be emitted as CommonJS"), "{err}");
    }

    #[test]
    fn commonjs_rejects_esm_export() {
        let err = transpile_single_err(
            "a.cts",
            "export const x: number = 1;\n",
            &commonjs_options(),
        );
        assert!(err.contains("cannot be emitted as CommonJS"), "{err}");
    }

    #[test]
    fn commonjs_rejects_top_level_await() {
        let err = transpile_single_err(
            "a.ts",
            "async function load(n: number): Promise<number> { return n; }\n\
             const value: number = await load(await load(1));\nexport = value;\n",
            &commonjs_options(),
        );
        assert!(
            err.contains("top-level await cannot be emitted as CommonJS"),
            "{err}"
        );
    }

    #[test]
    fn commonjs_rejects_top_level_for_await() {
        let err = transpile_single_err(
            "a.ts",
            "declare const items: AsyncIterable<Promise<number>>;\n\
             for await (const item of items) { await item; }\nexport = 1;\n",
            &commonjs_options(),
        );
        assert!(
            err.contains("top-level await cannot be emitted as CommonJS"),
            "{err}"
        );
    }

    #[test]
    fn commonjs_allows_await_inside_functions() {
        let js = transpile_js(
            "a.cts",
            "async function f(): Promise<number> { return await Promise.resolve(1); }\n\
             const g = async (): Promise<number> => await f();\nexport = g;\n",
            &commonjs_options(),
        );
        assert!(js.contains("module.exports = g"));
    }

    #[test]
    fn commonjs_rejects_top_level_await_using() {
        for init in ["r", "await r"] {
            let err = transpile_single_err(
                "a.ts",
                &format!(
                    "declare const r: AsyncDisposable;\nawait using x = {init};\nexport = x;\n"
                ),
                &commonjs_options(),
            );
            assert!(
                err.contains("top-level await cannot be emitted as CommonJS"),
                "{err}"
            );
        }
    }

    #[test]
    fn commonjs_allows_await_using_inside_functions() {
        let js = transpile_js(
            "a.cts",
            "declare const r: AsyncDisposable;\n\
             async function f(): Promise<void> { await using x = r; }\nexport = f;\n",
            &commonjs_options(),
        );
        assert!(js.contains("module.exports = f"), "js: {js}");
    }

    // import.meta is an expression, not a module-declaration statement, so it is caught via the module
    // record; Node rejects it in CommonJS files. (.ts source: the parser already rejects it in .cts.)
    #[test]
    fn commonjs_rejects_import_meta() {
        let err = transpile_single_err(
            "a.ts",
            "const url: string = import.meta.url;\nexport = url;\n",
            &commonjs_options(),
        );
        assert!(
            err.contains("import.meta cannot be emitted as CommonJS"),
            "{err}"
        );
    }

    #[test]
    fn preserve_keeps_esm_and_omits_use_strict() {
        let js = transpile_js("a.cts", "export const x: number = 1;\n", &js_options());
        assert!(js.contains("export const x = 1"), "js: {js}");
        assert!(!js.contains("use strict"), "js: {js}");
    }

    #[test]
    fn run_rejects_unsupported_module() {
        let err = run(args(&["--emit-js", "--module", "amd"])).unwrap_err();
        assert!(err[0].contains("unsupported --module \"amd\""), "{err:?}");
    }

    #[test]
    fn transpile_strips_types() {
        let js = transpile_js(
            "a.ts",
            "export const x: number = 1;\nexport interface I { a: string }\n",
            &default_options(),
        );
        assert!(js.contains("export const x = 1"), "js: {js}");
        assert!(!js.contains("interface"), "js: {js}");
    }

    // Enum members are evaluated up front: computed numeric members get their constant value and
    // string members get no reverse mapping, matching tsc.
    #[test]
    fn transpile_evaluates_enum_members() {
        let js = transpile_js(
            "a.ts",
            "export enum E { A = \"a\", B = 1, C, D = \"x\" + \"y\" }\n",
            &js_options(),
        );
        assert!(js.contains("E[\"A\"] = \"a\";"), "js: {js}");
        assert!(js.contains("E[E[\"C\"] = 2] = \"C\";"), "js: {js}");
        assert!(js.contains("E[\"D\"] = \"xy\";"), "js: {js}");
        assert!(!js.contains("= \"D\""), "js: {js}");
    }

    #[test]
    fn transpile_emits_declarations() {
        let result = transpile(
            "a.ts",
            "export function add(a: number, b: number): number { return a + b; }\n",
            &default_options(),
        )
        .unwrap();
        let dts = result.dts_code.unwrap();
        assert!(
            dts.contains("export declare function add(a: number, b: number): number"),
            "dts: {dts}"
        );
        assert!(!dts.contains("return"), "dts: {dts}");
    }

    const COMMENTED: &str = "\
/*! legal */
// note
/** Doc for x. */
export const x: number = 1;
";

    #[test]
    fn comments_kept_by_default() {
        let outputs = transpile("a.ts", COMMENTED, &default_options()).unwrap();
        let js = outputs.js_code.unwrap();
        assert!(js.contains("// note"), "js: {js}");
        assert!(js.contains("/** Doc for x. */"), "js: {js}");
        let dts = outputs.dts_code.unwrap();
        assert!(dts.contains("/** Doc for x. */"), "dts: {dts}");
    }

    #[test]
    fn remove_comments_strips_all_but_legal_comments() {
        let options = Options {
            remove_comments: true,
            ..default_options()
        };
        let outputs = transpile("a.ts", COMMENTED, &options).unwrap();
        let js = outputs.js_code.unwrap();
        assert!(js.contains("/*! legal */"), "js: {js}");
        assert!(!js.contains("note"), "js: {js}");
        assert!(!js.contains("Doc for x"), "js: {js}");
        let dts = outputs.dts_code.unwrap();
        assert!(!dts.contains("Doc for x"), "dts: {dts}");
    }

    #[test]
    fn remove_comments_keeps_pure_annotations() {
        let options = Options {
            remove_comments: true,
            ..js_options()
        };
        let js = transpile_js("a.tsx", "export const el = <div />;\n", &options);
        assert!(js.contains("/* @__PURE__ */"), "js: {js}");
    }

    #[test]
    fn strip_internal_omits_internal_declarations() {
        let src =
            "/** @internal */\nexport const secret: number = 1;\nexport const open: number = 2;\n";
        let kept = transpile("a.ts", src, &default_options()).unwrap();
        assert!(kept.dts_code.unwrap().contains("secret"));

        let options = Options {
            strip_internal: true,
            ..default_options()
        };
        let stripped = transpile("a.ts", src, &options).unwrap();
        let dts = stripped.dts_code.unwrap();
        assert!(!dts.contains("secret"), "dts: {dts}");
        assert!(dts.contains("open"), "dts: {dts}");
    }

    // Without the flag, imports left unused after type stripping are elided; with it they are
    // kept verbatim, like tsc's verbatimModuleSyntax. Type-only imports are removed either way.
    #[test]
    fn only_remove_type_imports_keeps_unused_imports() {
        let src = "import { sideEffect } from \"./fx.js\";\nimport type { T } from \"./t.js\";\nexport const x: number = 1;\n";
        let js = transpile_js("a.ts", src, &js_options());
        assert!(!js.contains("./fx.js"), "js: {js}");

        let options = Options {
            only_remove_type_imports: true,
            ..js_options()
        };
        let js = transpile_js("a.ts", src, &options);
        assert!(
            js.contains("import { sideEffect } from \"./fx.js\""),
            "js: {js}"
        );
        assert!(!js.contains("./t.js"), "js: {js}");
    }

    #[test]
    fn transpile_reports_parse_errors() {
        transpile("bad.ts", "const = ;", &default_options()).unwrap_err();
    }

    #[test]
    fn transpile_reports_isolated_declaration_errors() {
        transpile(
            "a.ts",
            "export function f() { return someValue(); }\nfunction someValue() { return 1; }\n",
            &default_options(),
        )
        .unwrap_err();
    }

    #[test]
    fn transpile_removes_class_fields_without_initializer() {
        let js = transpile_js(
            "a.ts",
            "export class C { declared: number; assigned = 1; }\n",
            &default_options(),
        );
        assert!(!js.contains("declared"), "js: {js}");
        assert!(js.contains("assigned = 1"), "js: {js}");
    }

    #[test]
    fn use_define_for_class_fields_keeps_field_definitions() {
        let options = Options {
            use_define_for_class_fields: true,
            ..js_options()
        };
        let js = transpile_js(
            "a.ts",
            "export class C { declared: number; assigned = 1; }\n",
            &options,
        );
        assert!(js.contains("declared"), "js: {js}");
        assert!(js.contains("assigned = 1"), "js: {js}");
    }

    #[test]
    fn source_maps_emitted_when_enabled() {
        let options = Options {
            source_maps: true,
            ..default_options()
        };
        let map = transpile("a.ts", "export const x: number = 1;\n", &options)
            .unwrap()
            .js_map
            .unwrap();
        assert!(map.contains("\"a.ts\""), "map: {map}");
    }

    #[test]
    fn inline_source_maps_produce_data_url() {
        let options = Options {
            inline_source_maps: true,
            ..js_options()
        };
        let outputs = transpile("a.ts", "export const x: number = 1;\n", &options).unwrap();
        let url = outputs.js_map.unwrap();
        assert!(
            url.starts_with("data:application/json;charset=utf-8;base64,"),
            "url: {url}"
        );
    }

    #[test]
    fn source_root_recorded_in_js_and_declaration_maps() {
        let options = Options {
            source_maps: true,
            declaration_maps: true,
            source_root: Some("/src".to_string()),
            ..default_options()
        };
        let outputs = transpile("a.ts", "export const x: number = 1;\n", &options).unwrap();
        for map in [outputs.js_map.unwrap(), outputs.dts_map.unwrap()] {
            assert!(map.contains("\"sourceRoot\":\"/src\""), "map: {map}");
        }
    }

    #[test]
    fn source_root_absent_by_default() {
        let options = Options {
            source_maps: true,
            ..js_options()
        };
        let outputs = transpile("a.ts", "export const x: number = 1;\n", &options).unwrap();
        let map = outputs.js_map.unwrap();
        assert!(!map.contains("sourceRoot"), "map: {map}");
    }

    #[test]
    fn run_source_root_dir_makes_sources_root_relative() {
        let dir = test_dir("run_source_root_dir");
        fs::create_dir_all(dir.join("pkg/src/sub")).unwrap();
        fs::create_dir_all(dir.join("out/sub")).unwrap();
        let src = dir.join("pkg/src/sub/a.ts");
        fs::write(&src, "export const x: number = 1;\n").unwrap();
        let js_out = dir.join("out/sub/a.js");
        run(args(&[
            "--emit-js",
            "--source-maps",
            "--source-root",
            "https://cdn.example/sources/",
            "--source-root-dir",
            dir.join("pkg/src").to_str().unwrap(),
            src.to_str().unwrap(),
            js_out.to_str().unwrap(),
        ]))
        .unwrap();
        let map = read(&dir.join("out/sub/a.js.map"));
        assert!(
            map.contains("\"sourceRoot\":\"https://cdn.example/sources/\""),
            "map: {map}"
        );
        assert!(map.contains("\"sources\":[\"sub/a.ts\"]"), "map: {map}");
    }

    #[test]
    fn run_rejects_source_root_dir_without_source_root() {
        let err = run(args(&[
            "--emit-js",
            "--source-maps",
            "--source-root-dir",
            "src",
        ]))
        .unwrap_err();
        assert_eq!(
            err,
            vec!["error: --source-root-dir requires --source-root".to_string()]
        );
    }

    #[test]
    fn run_rejects_source_maps_with_inline_source_maps() {
        let err = run(args(&[
            "--emit-js",
            "--source-maps",
            "--inline-source-maps",
        ]))
        .unwrap_err();
        assert_eq!(
            err,
            vec![
                "error: --source-maps and --inline-source-maps are mutually exclusive".to_string()
            ]
        );
    }

    #[test]
    fn run_rejects_source_root_without_maps() {
        let err = run(args(&["--emit-js", "--source-root", "/src"])).unwrap_err();
        assert!(
            err[0].starts_with("error: --source-root requires"),
            "{err:?}"
        );
    }

    #[test]
    fn declaration_maps_emitted_when_enabled() {
        let options = Options {
            declaration_maps: true,
            ..default_options()
        };
        let outputs = transpile("a.ts", "export const x: number = 1;\n", &options).unwrap();
        assert!(outputs.js_map.is_none());
        let map = outputs.dts_map.unwrap();
        assert!(map.contains("\"a.ts\""), "map: {map}");
        assert!(!map.contains("\"mappings\":\"\""), "map: {map}");
    }

    #[test]
    fn declaration_maps_not_emitted_by_default() {
        let outputs =
            transpile("a.ts", "export const x: number = 1;\n", &default_options()).unwrap();
        assert!(outputs.dts_map.is_none());
    }

    #[test]
    fn run_rejects_declaration_maps_without_emit_dts() {
        let err = run(args(&["--emit-js", "--declaration-maps"])).unwrap_err();
        assert_eq!(
            err,
            vec!["error: --declaration-maps requires --emit-dts".to_string()]
        );
    }

    #[test]
    fn transpile_plain_js() {
        let result = transpile("a.js", "export const x = 1;\n", &js_options()).unwrap();
        let js = result.js_code.unwrap();
        assert!(js.contains("export const x = 1"), "js: {js}");
        assert!(result.dts_code.is_none());
    }

    #[test]
    fn transpile_transforms_jsx() {
        let js = transpile_js(
            "a.tsx",
            "export const el: object = <div id={1} />;\n",
            &js_options(),
        );
        assert!(js.contains("react/jsx-runtime"), "js: {js}");
        assert!(js.contains("_jsx("), "js: {js}");
        assert!(!js.contains(": object"), "js: {js}");
    }

    #[test]
    fn transpile_transforms_jsx_in_plain_jsx_file() {
        let js = transpile_js(
            "a.jsx",
            "export const el = <div id={1} />;\n",
            &js_options(),
        );
        assert!(js.contains("_jsx("), "js: {js}");
    }

    // The classic runtime compiles JSX to React.createElement calls and imports nothing;
    // providing React is the caller's concern, as with tsc's jsx=react.
    #[test]
    fn jsx_classic_uses_create_element() {
        let options = Options {
            jsx: JsxRuntime::Classic,
            ..js_options()
        };
        let js = transpile_js(
            "a.tsx",
            "export const el: object = <div id={1} />;\n",
            &options,
        );
        assert!(js.contains("React.createElement"), "js: {js}");
        assert!(!js.contains("react/jsx-runtime"), "js: {js}");
        assert!(!js.contains("<div"), "js: {js}");
    }

    #[test]
    fn jsx_import_source_changes_runtime_module() {
        let options = Options {
            jsx_import_source: Some("preact".to_string()),
            ..js_options()
        };
        let js = transpile_js("a.tsx", "export const el = <div />;\n", &options);
        assert!(js.contains("\"preact/jsx-runtime\""), "js: {js}");
        assert!(!js.contains("\"react/jsx-runtime\""), "js: {js}");
    }

    // jsxFactory/jsxFragmentFactory: the classic runtime uses the given pragma, and the pragma's
    // import survives type stripping.
    #[test]
    fn jsx_pragma_changes_factory_and_preserves_pragma_import() {
        let options = Options {
            jsx: JsxRuntime::Classic,
            jsx_pragma: Some("h".to_string()),
            jsx_pragma_frag: Some("Fragment".to_string()),
            ..js_options()
        };
        let js = transpile_js(
            "a.tsx",
            "import { h, Fragment } from \"preact\";\nexport const el = <div><span /></div>;\nexport const frag = <></>;\n",
            &options,
        );
        assert!(js.contains("h(\"div\""), "js: {js}");
        assert!(js.contains("h(Fragment"), "js: {js}");
        assert!(
            js.contains("import { h, Fragment } from \"preact\""),
            "js: {js}"
        );
        assert!(!js.contains("React.createElement"), "js: {js}");
    }

    #[test]
    fn run_rejects_jsx_import_source_with_classic() {
        let err = run(args(&[
            "--emit-js",
            "--jsx",
            "classic",
            "--jsx-import-source",
            "preact",
        ]))
        .unwrap_err();
        assert_eq!(
            err,
            vec!["error: --jsx-import-source requires --jsx automatic".to_string()]
        );
    }

    #[test]
    fn run_rejects_jsx_pragma_with_automatic() {
        let err = run(args(&["--emit-js", "--jsx-pragma", "h"])).unwrap_err();
        assert_eq!(
            err,
            vec!["error: --jsx-pragma and --jsx-pragma-frag require --jsx classic".to_string()]
        );
    }

    #[test]
    fn run_rejects_unsupported_jsx() {
        let err = run(args(&["--emit-js", "--jsx", "react-jsx"])).unwrap_err();
        assert!(
            err[0].contains("unsupported --jsx \"react-jsx\""),
            "{err:?}"
        );
        let err = run(args(&["--emit-js", "--jsx", "preserve"])).unwrap_err();
        assert!(err[0].contains("unsupported --jsx \"preserve\""), "{err:?}");
    }

    // Specifiers are emitted verbatim, like tsc: no filesystem-based resolution of extensionless
    // or directory imports.
    #[test]
    fn transpile_emits_specifiers_verbatim() {
        let dir = test_dir("transpile_verbatim");
        write_file(&dir, "b.ts", "");
        fs::create_dir_all(dir.join("sub")).unwrap();
        write_file(&dir, "sub/index.ts", "");
        let src = dir.join("a.ts");
        let js = transpile_js(
            src.to_str().unwrap(),
            "export * from \"./b\";\nexport * from \"./sub\";\n",
            &js_options(),
        );
        assert!(js.contains("export * from \"./b\""), "js: {js}");
        assert!(js.contains("export * from \"./sub\""), "js: {js}");
        assert!(!js.contains("./b.js"), "js: {js}");
    }

    #[test]
    fn transpile_rewrites_dynamic_import_extension() {
        let dir = test_dir("transpile_dynamic_import_rewrite");
        let options = Options {
            rewrite_extensions: true,
            ..js_options()
        };
        let src = dir.join("a.ts");
        let js = transpile_js(
            src.to_str().unwrap(),
            "export const p = import(\"./b.ts\");\nexport async function f() { return await import(\"./c.mts\"); }\n",
            &options,
        );
        assert!(js.contains("import(\"./b.js\")"), "js: {js}");
        assert!(js.contains("import(\"./c.mjs\")"), "js: {js}");
    }

    // Dynamic imports only get the extension rewrite: extensionless specifiers are not resolved
    // against files on disk, unlike static imports.
    #[test]
    fn transpile_leaves_extensionless_dynamic_import() {
        let dir = test_dir("transpile_dynamic_import_extensionless");
        write_file(&dir, "b.ts", "");
        for rewrite_extensions in [false, true] {
            let options = Options {
                rewrite_extensions,
                ..js_options()
            };
            let src = dir.join("a.ts");
            let js = transpile_js(
                src.to_str().unwrap(),
                "export const p = import(\"./b\");\n",
                &options,
            );
            assert!(js.contains("import(\"./b\")"), "js: {js}");
        }
    }

    #[test]
    fn transpile_leaves_non_literal_dynamic_import() {
        let dir = test_dir("transpile_dynamic_import_non_literal");
        let options = Options {
            rewrite_extensions: true,
            ..js_options()
        };
        let src = dir.join("a.ts");
        let js = transpile_js(
            src.to_str().unwrap(),
            "export function f(name: string) { return import(name); }\n",
            &options,
        );
        assert!(js.contains("import(name)"), "js: {js}");
    }

    #[test]
    fn transpile_rewrites_static_import_extensions() {
        let options = Options {
            rewrite_extensions: true,
            ..js_options()
        };
        let js = transpile_js(
            "a.ts",
            "export { b } from \"./b.ts\";\nexport { c } from \"../c.cts\";\nexport { d } from \"./d.js\";\n",
            &options,
        );
        assert!(js.contains("\"./b.js\""), "js: {js}");
        assert!(js.contains("\"../c.cjs\""), "js: {js}");
        assert!(js.contains("\"./d.js\""), "js: {js}");
    }

    #[test]
    fn transpile_leaves_ts_extension_when_rewrite_disabled() {
        let js = transpile_js("a.ts", "export { b } from \"./b.ts\";\n", &js_options());
        assert!(js.contains("\"./b.ts\""));
    }

    #[test]
    fn transpile_rewrites_bare_specifier_with_slash() {
        let options = Options {
            rewrite_extensions: true,
            ..js_options()
        };
        let js = transpile_js("a.ts", "export { x } from \"pkg/x.ts\";\n", &options);
        assert!(js.contains("\"pkg/x.js\""));
    }

    #[test]
    fn transpile_reports_semantic_errors() {
        transpile("a.ts", "const a = 1;\nconst a = 2;\n", &default_options()).unwrap_err();
    }

    const DECORATED: &str = "\
function dec(_target: unknown, _key?: string): void {}
export class C {
  @dec
  method(x: string | null): void {}
}
";

    fn decorator_options() -> Options {
        Options {
            experimental_decorators: true,
            ..js_options()
        }
    }

    #[test]
    fn decorators_kept_without_experimental_decorators() {
        let js = transpile_js("a.ts", DECORATED, &js_options());
        assert!(js.contains("@dec"), "js: {js}");
        assert!(!js.contains("_decorate"), "js: {js}");
    }

    #[test]
    fn experimental_decorators_emit_legacy_helper_calls() {
        let js = transpile_js("a.ts", DECORATED, &decorator_options());
        assert!(!js.contains("@dec"), "js: {js}");
        assert!(js.contains("_decorate([dec]"), "js: {js}");
        assert!(
            js.contains("@oxc-project/runtime/helpers/decorate"),
            "js: {js}"
        );
        assert!(!js.contains("design:"), "js: {js}");
    }

    #[test]
    fn experimental_decorators_use_helpers_module() {
        let options = Options {
            helpers_module: Some("custom-helpers".to_string()),
            ..decorator_options()
        };
        let js = transpile_js("a.ts", DECORATED, &options);
        assert!(js.contains("custom-helpers/helpers/decorate"), "js: {js}");
        assert!(!js.contains("@oxc-project/runtime"), "js: {js}");
    }

    #[test]
    fn emit_decorator_metadata_records_design_types() {
        let options = Options {
            emit_decorator_metadata: true,
            ..decorator_options()
        };
        let js = transpile_js("a.ts", DECORATED, &options);
        assert!(
            js.contains("_decorateMetadata(\"design:type\", Function)"),
            "js: {js}"
        );
        assert!(
            js.contains("_decorateMetadata(\"design:paramtypes\", [Object])"),
            "js: {js}"
        );
        assert!(
            js.contains("_decorateMetadata(\"design:returntype\", void 0)"),
            "js: {js}"
        );
    }

    // With strictNullChecks off, `string | null` is just string to tsc, so the metadata records
    // String rather than the Object it uses for unions.
    #[test]
    fn no_strict_null_checks_unwraps_nullable_metadata() {
        let options = Options {
            emit_decorator_metadata: true,
            no_strict_null_checks: true,
            ..decorator_options()
        };
        let js = transpile_js("a.ts", DECORATED, &options);
        assert!(
            js.contains("_decorateMetadata(\"design:paramtypes\", [String])"),
            "js: {js}"
        );
    }

    #[test]
    fn run_rejects_metadata_without_experimental_decorators() {
        let err = run(args(&["--emit-js", "--emit-decorator-metadata"])).unwrap_err();
        assert_eq!(
            err,
            vec!["error: --emit-decorator-metadata requires --experimental-decorators".to_string()]
        );
    }

    #[test]
    fn transpile_supports_mts_and_cts_sources() {
        let result =
            transpile("a.mts", "export const x: number = 1;\n", &default_options()).unwrap();
        assert!(result.js_code.unwrap().contains("export const x = 1"));
        assert!(result.dts_code.unwrap().contains("declare const x: number"));

        let result =
            transpile("a.cts", "export const y: number = 2;\n", &default_options()).unwrap();
        assert!(result.js_code.unwrap().contains("y = 2"));
        assert!(result.dts_code.is_some());
    }

    #[test]
    fn run_rejects_unknown_flag() {
        let err = run(args(&["--emit-js", "--sourcemap", "a.ts", "a.js"])).unwrap_err();
        assert_eq!(err, vec!["error: unknown flag \"--sourcemap\"".to_string()]);
    }

    #[test]
    fn run_requires_emit_flag() {
        let err = run(args(&[])).unwrap_err();
        assert_eq!(
            err,
            vec!["error: at least one of --emit-js or --emit-dts is required".to_string()]
        );
    }

    #[test]
    fn run_requires_positive_cpu_count() {
        for value in ["0", "-1", "many"] {
            let err = run(args(&["--emit-js", "--cpus", value])).unwrap_err();
            assert_eq!(
                err,
                vec![format!(
                    "error: --cpus must be a positive integer, got \"{value}\""
                )]
            );
        }
    }

    #[test]
    fn run_reports_unreadable_manifest() {
        let dir = test_dir("run_missing_manifest");
        let err = run_in(&dir, &["--emit-js", "--manifest"], ["missing.txt"]).unwrap_err();
        assert!(err[0].starts_with("error: cannot read manifest"), "{err:?}");
    }

    #[test]
    fn run_rejects_entries_missing_enabled_output_paths() {
        let dir = test_dir("run_misaligned");
        write_file(&dir, "a.ts", "export const x = 1;\n");
        let err = run_in(&dir, &["--emit-js", "--emit-dts"], ["a.ts", "a.js"]).unwrap_err();
        assert!(err[0].contains("expected entries of 3 lines"), "{err:?}");
    }

    #[test]
    fn run_transpiles_manifest_entries() {
        let dir = test_dir("run_manifest");
        let src = write_file(&dir, "a.ts", "export const x: number = 1;\n");
        let js_out = dir.join("out/a.js");
        let dts_out = dir.join("out/a.d.ts");
        write_file(
            &dir,
            "manifest.txt",
            &format!(
                "{}\n{}\n{}\n",
                src.display(),
                js_out.display(),
                dts_out.display()
            ),
        );
        run_in(
            &dir,
            &["--emit-js", "--emit-dts", "--manifest"],
            ["manifest.txt"],
        )
        .unwrap();
        assert!(read(&js_out).contains("export const x = 1"));
        assert!(read(&dts_out).contains("declare const x: number"));
    }

    #[test]
    fn run_transpiles_positional_entries() {
        let dir = test_dir("run_positional");
        write_file(&dir, "a.ts", "export const x: number = 1;\n");
        run_in(&dir, &["--emit-js"], ["a.ts", "a.out.js"]).unwrap();
        assert!(read(&dir.join("a.out.js")).contains("export const x = 1"));
    }

    #[test]
    fn run_transpiles_multiple_entries_with_requested_cpu_count() {
        let dir = test_dir("run_parallel");
        write_file(&dir, "first.ts", "export const first: number = 1;\n");
        write_file(&dir, "second.ts", "export const second: number = 2;\n");
        run_in(
            &dir,
            &["--emit-js", "--cpus", "2"],
            ["first.ts", "out/first.js", "second.ts", "out/second.js"],
        )
        .unwrap();
        assert!(read(&dir.join("out/first.js")).contains("export const first = 1"));
        assert!(read(&dir.join("out/second.js")).contains("export const second = 2"));
    }

    #[test]
    fn run_reports_parallel_errors_in_manifest_order() {
        let dir = test_dir("run_parallel_error_order");
        let mut paths = Vec::new();
        for i in 0..6 {
            write_file(&dir, &format!("{i}.ts"), "export const x: number = ;\n");
            paths.push(format!("{i}.ts"));
            paths.push(format!("out/{i}.js"));
        }
        let err = run_in(&dir, &["--emit-js", "--cpus", "2"], &paths).unwrap_err();
        let reported: Vec<_> = err
            .iter()
            .filter_map(|e| e.split('/').last()?.split(':').next().map(str::to_string))
            .collect();
        let expected: Vec<_> = (0..6).map(|i| format!("{i}.ts")).collect();
        assert_eq!(reported, expected, "{err:?}");
    }

    #[test]
    fn run_skips_entries_with_empty_output_path() {
        let dir = test_dir("run_empty_dts");
        let src = write_file(&dir, "a.js", "export const x = 1;\n");
        let js_out = dir.join("out/a.js");
        write_file(
            &dir,
            "manifest.txt",
            &format!("{}\n{}\n\n", src.display(), js_out.display()),
        );
        run_in(
            &dir,
            &["--emit-js", "--emit-dts", "--manifest"],
            ["manifest.txt"],
        )
        .unwrap();
        assert!(read(&js_out).contains("export const x = 1"));
        assert!(!dir.join("out/a.d.ts").exists());
    }

    // Declaration files are never transpile entries: matching tsc, they are inputs to the type
    // checker, not emitted outputs.
    #[test]
    fn run_rejects_declaration_file_entries() {
        let dir = test_dir("run_dts_rejected");
        for name in ["a.d.ts", "a.d.mts", "a.d.cts"] {
            let src = write_file(&dir, name, "export declare const x: number;\n");
            let dts_out = dir.join(format!("out/{name}"));
            write_file(
                &dir,
                "manifest.txt",
                &format!("{}\n\n{}\n", src.display(), dts_out.display()),
            );
            let err = run_in(
                &dir,
                &["--emit-js", "--emit-dts", "--manifest"],
                ["manifest.txt"],
            )
            .unwrap_err();
            assert!(err[0].contains("produces no outputs"), "{err:?}");
            assert!(!dts_out.exists());
        }
    }

    #[test]
    fn run_writes_external_source_map_with_relative_source_path() {
        let dir = test_dir("run_source_maps");
        write_file(&dir, "a.ts", "export const x: number = 1;\n");
        run_in(&dir, &["--emit-js", "--source-maps"], ["a.ts", "out/a.js"]).unwrap();
        let js = read(&dir.join("out/a.js"));
        assert!(
            js.ends_with("= 1;\n//# sourceMappingURL=a.js.map"),
            "js: {js}"
        );
        let map = read(&dir.join("out/a.js.map"));
        assert!(map.contains("\"sources\":[\"../a.ts\"]"), "map: {map}");
    }

    #[test]
    fn run_embeds_inline_source_map() {
        let dir = test_dir("run_inline_source_maps");
        let src = dir.join("a.ts");
        fs::write(&src, "export const x: number = 1;\n").unwrap();
        let js_out = dir.join("out/a.js");
        run(args(&[
            "--emit-js",
            "--inline-source-maps",
            src.to_str().unwrap(),
            js_out.to_str().unwrap(),
        ]))
        .unwrap();
        let js = read(&js_out);
        assert!(
            js.contains("\n//# sourceMappingURL=data:application/json;charset=utf-8;base64,"),
            "js: {js}"
        );
        assert!(!dir.join("out/a.js.map").exists());
    }

    #[test]
    fn run_writes_declaration_map_relative_to_declaration_dir() {
        let dir = test_dir("run_declaration_maps");
        write_file(&dir, "a.ts", "export const x: number = 1;\n");
        fs::create_dir_all(dir.join("types")).unwrap();
        run_in(
            &dir,
            &["--emit-js", "--emit-dts", "--declaration-maps"],
            ["a.ts", "out/a.js", "types/a.d.ts"],
        )
        .unwrap();
        assert!(!dir.join("out/a.js.map").exists());
        assert!(!read(&dir.join("out/a.js")).contains("sourceMappingURL"));
        let dts = read(&dir.join("types/a.d.ts"));
        assert!(
            dts.ends_with("//# sourceMappingURL=a.d.ts.map"),
            "dts: {dts}"
        );
        let map = read(&dir.join("types/a.d.ts.map"));
        assert!(map.contains("\"sources\":[\"../a.ts\"]"), "map: {map}");
    }

    #[test]
    fn path_relative_to_walks_up_to_common_ancestor() {
        assert_eq!(
            path_relative_to(Path::new("main.ts"), Path::new(".")),
            PathBuf::from("main.ts")
        );
        assert_eq!(
            path_relative_to(
                Path::new("pkg/src/a.ts"),
                Path::new("bazel-out/bin/pkg/dist")
            ),
            PathBuf::from("../../../../pkg/src/a.ts")
        );
        assert_eq!(
            path_relative_to(Path::new("pkg/src/a.ts"), Path::new("pkg/src")),
            PathBuf::from("a.ts")
        );
        assert_eq!(
            path_relative_to(Path::new("pkg/src/a.ts"), Path::new("pkg/dist/sub")),
            PathBuf::from("../../src/a.ts")
        );
    }

    #[test]
    fn run_writes_successful_entries_while_aggregating_errors() {
        let dir = test_dir("run_error_aggregation");
        write_file(&dir, "bad1.ts", "const = ;");
        write_file(&dir, "bad2.ts", "const = ;");
        write_file(&dir, "good.ts", "export const x = 1;\n");
        let err = run_in(
            &dir,
            &["--emit-js"],
            [
                "bad1.ts",
                "out/bad1.js",
                "bad2.ts",
                "out/bad2.js",
                "good.ts",
                "out/good.js",
            ],
        )
        .unwrap_err();
        assert!(err.len() >= 2, "errors: {err:?}");
        assert!(dir.join("out/good.js").exists());
    }

    #[test]
    fn run_writes_js_maps_and_declarations_before_later_errors() {
        let dir = test_dir("run_streams_all_output_kinds");
        write_file(&dir, "good.ts", "export const x: number = 1;\n");
        write_file(&dir, "bad.ts", "const = ;");
        let err = run_in(
            &dir,
            &["--emit-js", "--emit-dts", "--source-maps"],
            [
                "good.ts",
                "out/good.js",
                "out/good.d.ts",
                "bad.ts",
                "out/bad.js",
                "out/bad.d.ts",
            ],
        )
        .unwrap_err();

        assert!(!err.is_empty(), "errors: {err:?}");
        assert!(dir.join("out/good.js").exists());
        assert!(dir.join("out/good.js.map").exists());
        assert!(dir.join("out/good.d.ts").exists());
    }

    #[test]
    fn run_reports_unwritable_output() {
        let dir = test_dir("run_unwritable_output");
        write_file(&dir, "a.ts", "export const x: number = 1;\n");
        write_file(&dir, "blocker", "");
        let err = run_in(&dir, &["--emit-js"], ["a.ts", "blocker/a.js"]).unwrap_err();
        assert!(err[0].starts_with("error: cannot write"), "{err:?}");
    }

    #[test]
    fn run_does_not_create_output_directories() {
        let dir = test_dir("run_missing_output_dir");
        write_file(&dir, "a.ts", "export const x: number = 1;\n");
        let err = run_in(&dir, &["--emit-js"], ["a.ts", "missing/a.js"]).unwrap_err();
        assert!(err[0].starts_with("error: cannot write"), "{err:?}");
        assert!(!dir.join("missing").exists());
    }

    #[test]
    fn run_aggregates_read_errors_across_entries() {
        let dir = test_dir("run_read_error_aggregation");
        write_file(&dir, "bad.ts", "const = ;");
        let err = run_in(
            &dir,
            &["--emit-js"],
            ["missing.ts", "out/missing.js", "bad.ts", "out/bad.js"],
        )
        .unwrap_err();
        assert!(err.len() >= 2, "errors: {err:?}");
        assert!(err[0].contains("cannot read"), "errors: {err:?}");
    }
}
