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
    CompilerAssumptions, DecoratorOptions, EnvOptions, HelperLoaderOptions, JsxOptions,
    JsxRuntime, Module, RewriteExtensionsMode, TransformOptions, Transformer, TypeScriptOptions,
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
    // tsc's inlineSourceMap: the JS source map embedded as a data URL in the sourceMappingURL
    // comment instead of written to a .js.map file. Exclusive with source_maps.
    inline_source_maps: bool,
    // tsc's declarationMap: a .d.ts.map beside each declaration output. Requires emit_dts.
    declaration_maps: bool,
    // tsc's sourceRoot: the `sourceRoot` recorded in every source map.
    source_root: Option<String>,
    // Directory the map `sources` are recorded relative to when source_root is set, since
    // consumers then resolve them against source_root instead of the map's location. tsc uses
    // its rootDir here; the Bazel rule passes the root_dir. Without it sources stay relative to
    // the map, which is only correct when the map sits in that root.
    source_root_dir: Option<PathBuf>,
    // tsc's removeComments: drop comments from the JS and declaration outputs, keeping legal
    // comments (`/*!`, @license, @preserve) like tsc and the annotations tooling relies on
    // (`/* @__PURE__ */` and friends).
    remove_comments: bool,
    // The "automatic" or "classic" runtime of the JSX transform (tsc's jsx=react-jsx and
    // jsx=react). No jsx=preserve equivalent: preserved JSX cannot run under Node, and a
    // JSX-aware bundler consumes the .tsx sources directly.
    jsx: JsxRuntime,
    rewrite_extensions: bool,
    // Downlevel transforms for an ES target (e.g. "es2017"), like tsc's `target`; None keeps the latest syntax.
    env: Option<EnvOptions>,
    // Module to import runtime helpers from, defaulting to @oxc-project/runtime. Helpers are
    // always imported (like tsc's importHelpers): oxc has no inline helper mode.
    helpers_module: Option<String>,
    // Module format of the output, using oxc's Module values. Preserve keeps the input syntax.
    // Esm makes TypeScript's CommonJS-specific syntax (`export =`, `import x = require(...)`) an
    // error. CommonJS rewrites that syntax and adds "use strict"; oxc has no ESM-to-CJS
    // transform, so remaining ESM syntax is an error.
    module: Module,
    // Module the automatic JSX runtime is imported from (oxc's import_source), tsc's
    // jsxImportSource. Defaults to "react".
    jsx_import_source: Option<String>,
    // Factory and fragment for the classic JSX runtime (oxc's pragma/pragma_frag), tsc's
    // jsxFactory/jsxFragmentFactory. Default to React.createElement and React.Fragment.
    jsx_pragma: Option<String>,
    jsx_pragma_frag: Option<String>,
    // tsc's useDefineForClassFields. false (the default, matching swc) maps to oxc's
    // set_public_class_fields assumption plus remove_class_fields_without_initializer; true keeps
    // define semantics, tsc's own default for target >= es2022.
    use_define_for_class_fields: bool,
    // tsc's stripInternal: omit declarations marked /** @internal */ from the .d.ts outputs.
    strip_internal: bool,
    // tsc's verbatimModuleSyntax: only imports/exports marked `type` are removed, instead of
    // eliding any import that is unused after type stripping.
    only_remove_type_imports: bool,
    // tsc's experimentalDecorators: the legacy (pre-TC39) decorator transform, emitting
    // _decorate/_decorateParam helper calls. Without it decorators are left as written, since
    // oxc has no transform for the standard proposal. Helpers come from the helpers module.
    experimental_decorators: bool,
    // tsc's emitDecoratorMetadata: design:type/paramtypes/returntype metadata via
    // _decorateMetadata, which calls Reflect.metadata at runtime (a reflect-metadata polyfill
    // is the caller's concern). Requires experimental_decorators.
    emit_decorator_metadata: bool,
    // tsc's strictNullChecks=false, which only affects decorator metadata: `T | null` records
    // T's constructor rather than Object.
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

// The value following `flag`, or an error naming the flag when missing.
fn flag_value(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
    what: &str,
) -> Result<String, Vec<String>> {
    args.next().ok_or_else(|| vec![format!("error: {flag} requires {what}")])
}

fn run(args: impl Iterator<Item = String>) -> Result<(), Vec<String>> {
    let Cli { options, cpus, manifest_path, positional } = parse_args(args)?;
    let entries = read_entries(&options, manifest_path, positional)?;
    transpile_entries(&options, cpus, &entries)
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
                cli.options.source_root = Some(flag_value(&mut args, &arg, "a value")?);
            }
            "--source-root-dir" => {
                cli.options.source_root_dir =
                    Some(PathBuf::from(flag_value(&mut args, &arg, "a directory path")?));
            }
            "--remove-comments" => cli.options.remove_comments = true,
            "--cpus" => {
                let value = flag_value(&mut args, &arg, "a positive integer")?;
                cli.cpus = value
                    .parse()
                    .ok()
                    .filter(|cpus: &usize| *cpus > 0)
                    .ok_or_else(|| {
                        vec![format!("error: --cpus must be a positive integer, got \"{value}\"")]
                    })?;
            }
            "--jsx" => {
                let value = flag_value(&mut args, &arg, "a value")?;
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
                cli.options.jsx_import_source = Some(flag_value(&mut args, &arg, "a value")?);
            }
            "--jsx-pragma" => {
                cli.options.jsx_pragma = Some(flag_value(&mut args, &arg, "a value")?);
            }
            "--jsx-pragma-frag" => {
                cli.options.jsx_pragma_frag = Some(flag_value(&mut args, &arg, "a value")?);
            }
            "--rewrite-extensions" => cli.options.rewrite_extensions = true,
            "--use-define-for-class-fields" => cli.options.use_define_for_class_fields = true,
            "--strip-internal" => cli.options.strip_internal = true,
            "--only-remove-type-imports" => cli.options.only_remove_type_imports = true,
            "--experimental-decorators" => cli.options.experimental_decorators = true,
            "--emit-decorator-metadata" => cli.options.emit_decorator_metadata = true,
            "--no-strict-null-checks" => cli.options.no_strict_null_checks = true,
            "--target" => {
                let target = flag_value(&mut args, &arg, "a value")?;
                // Repeats the Bazel rule's attr allowlist: only whole ES versions that oxc can
                // fully downlevel to. Rejects es5 (oxc cannot fully downlevel ES2015 syntax) and
                // engine/browserslist targets, which EnvOptions::from_target would accept.
                if !SUPPORTED_TARGETS.contains(&target.as_str()) {
                    return Err(vec![format!(
                        "error: unsupported --target \"{target}\": expected es6, es2015..es2026, or esnext"
                    )]);
                }
                cli.options.env = Some(EnvOptions::from_target(&target).expect("allowlisted target"));
            }
            "--helpers-module" => {
                cli.options.helpers_module = Some(flag_value(&mut args, &arg, "a value")?);
            }
            "--module" => {
                let value = flag_value(&mut args, &arg, "a value")?;
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
                cli.manifest_path = Some(flag_value(&mut args, &arg, "a file path")?);
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
        return Err(vec!["error: --declaration-maps requires --emit-dts".to_string()]);
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
        return Err(vec!["error: --source-root-dir requires --source-root".to_string()]);
    }

    Ok(cli)
}

// Each manifest entry is the source path followed by the JS output path (when --emit-js) and
// the declaration output path (when --emit-dts). An empty output path skips that output for
// the entry (e.g. no declarations for plain JS sources). Entries come from the manifest file
// when given, otherwise from the positional arguments.
fn read_entries(
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
        let mut output =
            |emit: bool| emit.then(|| lines.next().unwrap()).filter(|out| !out.is_empty());
        let js_out = output(options.emit_js);
        let dts_out = output(options.emit_dts);
        entries.push(Entry { src, js_out, dts_out });
    }
    Ok(entries)
}

// Transpiles and writes every entry, with up to --cpus entries in flight at once. Failures do
// not prevent other entries from being processed, so all errors are reported in one pass, in
// manifest order. Outputs from entries that completed before a failure are intentionally
// retained rather than buffering every generated file in memory.
fn transpile_entries(options: &Options, cpus: usize, entries: &[Entry]) -> Result<(), Vec<String>> {
    let transform_options = build_transform_options(options);
    let next = AtomicUsize::new(0);
    let mut errors: Vec<(usize, Vec<String>)> = thread::scope(|scope| {
        let workers: Vec<_> = (0..cpus.min(entries.len()))
            .map(|_| {
                scope.spawn(|| {
                    // One arena per worker, reset between files instead of reallocated.
                    let mut allocator = Allocator::default();
                    let mut errors = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(entry) = entries.get(i) else { break errors };
                        let entry_errors =
                            transpile_entry(options, &transform_options, &mut allocator, entry);
                        if !entry_errors.is_empty() {
                            errors.push((i, entry_errors));
                        }
                    }
                })
            })
            .collect();
        workers.into_iter().flat_map(|worker| worker.join().unwrap()).collect()
    });
    errors.sort_by_key(|(i, _)| *i);

    let all_errors: Vec<String> = errors.into_iter().flat_map(|(_, e)| e).collect();
    if all_errors.is_empty() { Ok(()) } else { Err(all_errors) }
}

fn transpile_entry(
    options: &Options,
    transform_options: &TransformOptions,
    allocator: &mut Allocator,
    entry: &Entry,
) -> Vec<String> {
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

    // Source map consumers resolve `sources` against the map's own location — or, when
    // sourceRoot is set, against that root, so sources are recorded relative to its directory.
    let map_source = |out: &Option<String>| {
        if let Some(root_dir) = &options.source_root_dir {
            return relative_to(Path::new(&entry.src), root_dir);
        }
        out.as_deref()
            .and_then(|out| Path::new(out).parent())
            .map(|out_dir| relative_to(Path::new(&entry.src), out_dir))
            .unwrap_or_else(|| PathBuf::from(&entry.src))
    };
    let outputs = match transpile_to(
        allocator,
        transform_options,
        &entry.src,
        &content,
        options,
        entry.js_out.is_some(),
        entry.dts_out.is_some(),
        &map_source(&entry.js_out),
        &map_source(&entry.dts_out),
    ) {
        Ok(outputs) => outputs,
        Err(errors) => return errors,
    };

    let mut errors = Vec::new();
    if let (Some(js_out), Some(code)) = (&entry.js_out, outputs.js_code) {
        let map = if options.inline_source_maps {
            outputs.js_map_data_url.map(SourceMapOutput::Inline)
        } else {
            outputs.js_map.map(SourceMapOutput::File)
        };
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

enum SourceMapOutput {
    // JSON written to a sibling .map file, referenced by name.
    File(String),
    // A data URL embedded in the sourceMappingURL comment itself.
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
                write!(file, "//# sourceMappingURL={}.map", out.file_name().unwrap().to_string_lossy())?;
                fs::write(format!("{path}.map"), json)?;
            }
            SourceMapOutput::Inline(url) => write!(file, "//# sourceMappingURL={url}")?,
        }
    }
    Ok(())
}

// `target` expressed relative to `base_dir`, with both paths relative to the same root (or both
// absolute), as Bazel passes exec-root-relative source and output paths.
fn relative_to(target: &Path, base_dir: &Path) -> PathBuf {
    let mut target_parts = target.components().peekable();
    let mut base_parts = base_dir.components().peekable();
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

// Matches the Bazel rule's _is_declaration: all three declaration extensions, so a .d.mts/.d.cts
// input passes through instead of hitting the transpile path (isolated declarations would error
// on a declaration file).
fn is_declaration_file(path: &str) -> bool {
    path.ends_with(".d.ts") || path.ends_with(".d.mts") || path.ends_with(".d.cts")
}

#[derive(Debug, Default)]
struct Outputs {
    js_code: Option<String>,
    js_map: Option<String>,
    js_map_data_url: Option<String>,
    dts_code: Option<String>,
    dts_map: Option<String>,
}

fn codegen_options(options: &Options, source_map_path: Option<PathBuf>) -> CodegenOptions {
    CodegenOptions {
        source_map_path,
        comments: if options.remove_comments {
            CommentOptions { normal: false, jsdoc: false, ..CommentOptions::default() }
        } else {
            CommentOptions::default()
        },
        ..Default::default()
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

fn build_transform_options(options: &Options) -> TransformOptions {
    TransformOptions {
        // Without --use-define-for-class-fields, matches tsc's useDefineForClassFields=false:
        // fields are assigned with `=` rather than Object.defineProperty, and fields without an
        // initializer are removed rather than set to undefined. With the flag, define semantics
        // are kept (tsc's own default for target >= es2022).
        assumptions: CompilerAssumptions {
            set_public_class_fields: !options.use_define_for_class_fields,
            ..Default::default()
        },
        typescript: {
            let mut typescript = TypeScriptOptions {
                remove_class_fields_without_initializer: !options.use_define_for_class_fields,
                // Like tsc's verbatimModuleSyntax: keep imports that are unused after type
                // stripping.
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
        // Helpers are imported like tsc's importHelpers (oxc's only implemented mode);
        // --helpers-module redirects the imports away from the default @oxc-project/runtime.
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
        walk::walk_await_expression(self, expr);
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
        }
        walk::walk_variable_declaration(self, decl);
    }

    // Await inside any function belongs to that function, not the module.
    fn visit_function(&mut self, _func: &Function<'a>, _flags: ScopeFlags) {}

    fn visit_arrow_function_expression(&mut self, _func: &ArrowFunctionExpression<'a>) {}
}

// An `export {}` with no specifiers: it only marks the file as a module and has no CommonJS
// equivalent. (Exports with a declaration or a source are separate AST variants.)
fn is_empty_export(stmt: &Statement) -> bool {
    match stmt {
        Statement::ExportNamedDeclaration(decl) => decl.specifiers.is_empty(),
        _ => false,
    }
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

fn render_errors(
    filename: &str,
    source_text: &str,
    diagnostics: impl IntoIterator<Item = OxcDiagnostic>,
) -> Vec<String> {
    let handler = GraphicalReportHandler::new().with_theme(GraphicalTheme::none());
    // Arc-shared so each diagnostic does not clone the whole source text.
    let source = std::sync::Arc::new(NamedSource::new(filename, source_text.to_string()));
    diagnostics
        .into_iter()
        .map(|diagnostic| {
            let diagnostic = diagnostic.with_source_code(source.clone());
            let mut s = String::new();
            handler.render_report(&mut s, diagnostic.as_ref()).unwrap();
            s
        })
        .collect()
}

// Parses once and emits JS and/or declaration outputs from the same AST. Declarations are emitted
// first, before the transformer mutates the program. `js_map_source` and `dts_map_source` are the
// paths recorded in the respective source maps' `sources`.
fn transpile_to(
    allocator: &mut Allocator,
    transform_options: &TransformOptions,
    filename: &str,
    source_text: &str,
    options: &Options,
    emit_js: bool,
    emit_dts: bool,
    js_map_source: &Path,
    dts_map_source: &Path,
) -> Result<Outputs, Vec<String>> {
    // Every source is parsed as TypeScript, including plain .js/.jsx: one pipeline for all inputs.
    let source_type = SourceType::from_path(filename)
        .unwrap_or_default()
        .with_typescript(true);

    allocator.reset();
    let allocator = &*allocator;
    let mut parser_ret = Parser::new(allocator, source_text, source_type).parse();

    let mut outputs = Outputs::default();

    if !parser_ret.diagnostics.is_empty() {
        return Err(render_errors(filename, source_text, parser_ret.diagnostics));
    }

    if emit_dts {
        let decl_ret = IsolatedDeclarations::new(
            allocator,
            OxcIsolatedDeclarationsOptions {
                strip_internal: options.strip_internal,
            },
        )
        .build(&parser_ret.program);

        if !decl_ret.diagnostics.is_empty() {
            return Err(render_errors(filename, source_text, decl_ret.diagnostics));
        }

        let codegen_ret = Codegen::new()
            .with_options(codegen_options(
                options,
                options.declaration_maps.then(|| dts_map_source.to_path_buf()),
            ))
            .build(&decl_ret.program);
        outputs.dts_code = Some(codegen_ret.code);
        outputs.dts_map = with_source_root!(codegen_ret.map, options).map(|m| m.to_json_string());
    }

    if emit_js {
        // oxc's own compiler configuration: the transformer needs pre-evaluated enum member
        // values (string members are otherwise given reverse mappings), and roughly triples the
        // scope and symbol counts.
        let semantic_ret = SemanticBuilder::new_compiler()
            .with_enum_eval(true)
            .with_excess_capacity(2.0)
            .build(&parser_ret.program);
        let semantic_diagnostics = semantic_ret.diagnostics;
        let scoping = semantic_ret.semantic.into_scoping();

        let transformer_ret = Transformer::new(allocator, Path::new(filename), transform_options)
            .build_with_scoping(scoping, &mut parser_ret.program);

        let diagnostics: Vec<_> = semantic_diagnostics
            .into_iter()
            .chain(transformer_ret.diagnostics)
            .collect();
        if !diagnostics.is_empty() {
            return Err(render_errors(filename, source_text, diagnostics));
        }

        if options.module.is_commonjs() {
            // An empty `export {}` — hand-written, or appended by the transformer after erasing a
            // file's only module syntax — is dropped rather than rejected, matching tsc.
            parser_ret.program.body.retain(|stmt| !is_empty_export(stmt));
            let diagnostics =
                commonjs_diagnostics(&parser_ret.program, &parser_ret.module_record);
            if !diagnostics.is_empty() {
                return Err(render_errors(filename, source_text, diagnostics));
            }
        }

        let codegen_ret = Codegen::new()
            .with_options(codegen_options(
                options,
                (options.source_maps || options.inline_source_maps)
                    .then(|| js_map_source.to_path_buf()),
            ))
            .build(&parser_ret.program);

        outputs.js_code = Some(codegen_ret.code);
        // Only the emitted form is serialized: the data URL for inline maps, JSON otherwise.
        let map = with_source_root!(codegen_ret.map, options);
        if options.inline_source_maps {
            outputs.js_map_data_url = map.as_ref().map(|m| m.to_data_url());
        } else {
            outputs.js_map = map.map(|m| m.to_json_string());
        }
    }

    Ok(outputs)
}


#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let base = std::env::var_os("TEST_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = base.join(name);
        // Outputs go under out/, created up front like Bazel does for declared outputs.
        fs::create_dir_all(dir.join("out")).unwrap();
        dir
    }

    fn args(list: &[&str]) -> impl Iterator<Item = String> {
        list.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    fn transpile(
        filename: &str,
        source_text: &str,
        options: &Options,
    ) -> Result<Outputs, Vec<String>> {
        let Options { emit_js, emit_dts, .. } = *options;
        transpile_to(
            &mut Allocator::default(),
            &build_transform_options(options),
            filename,
            source_text,
            options,
            emit_js,
            emit_dts,
            Path::new(filename),
            Path::new(filename),
        )
    }

    fn default_options() -> Options {
        Options { emit_js: true, emit_dts: true, ..Options::default() }
    }

    fn js_options() -> Options {
        Options { emit_dts: false, ..default_options() }
    }

    fn target_options(target: &str) -> Options {
        Options {
            env: Some(EnvOptions::from_target(target).unwrap()),
            ..js_options()
        }
    }

    fn commonjs_options() -> Options {
        Options { module: Module::CommonJS, ..js_options() }
    }

    fn esm_options() -> Options {
        Options { module: Module::Esm, ..js_options() }
    }

    // JS output of a successful transpile; panics with the rendered errors otherwise.
    fn transpile_js(filename: &str, source_text: &str, options: &Options) -> String {
        transpile(filename, source_text, options).unwrap().js_code.unwrap()
    }

    #[test]
    fn target_downlevels_exponentiation() {
        let js = transpile_js("a.ts", "export const x: number = 2 ** 10;\n", &target_options("es2015"));
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

    // Downleveled async functions need the asyncToGenerator helper, imported from @oxc-project/runtime:
    // that package must be a runtime dependency when targets below es2017 are used with async code.
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
            // Parsing succeeds; the error is the unrelated missing-emit check.
            assert_eq!(
                err,
                vec!["error: at least one of --emit-js or --emit-dts is required".to_string()],
                "target {target}"
            );
        }
    }

    #[test]
    fn run_requires_target_value() {
        let err = run(args(&["--emit-js", "--target"])).unwrap_err();
        assert_eq!(err, vec!["error: --target requires a value".to_string()]);
    }

    // Helpers only appear when a transform needs one, and none does in the default configuration;
    // downlevel async to es2016 so the transform pulls in the asyncToGenerator helper.
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
        assert!(js.contains("\"@oxc-project/runtime/helpers/asyncToGenerator\""), "js: {js}");
    }

    #[test]
    fn helpers_module_redirects_helper_imports() {
        let js = transpile_js(
            "a.ts",
            "export async function f(): Promise<number> { return 1; }\n",
            &helpers_options(Some("custom-helpers")),
        );
        assert!(js.contains("\"custom-helpers/helpers/asyncToGenerator\""), "js: {js}");
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
    fn run_requires_helpers_module_value() {
        let err = run(args(&["--emit-js", "--helpers-module"])).unwrap_err();
        assert_eq!(err, vec!["error: --helpers-module requires a value".to_string()]);
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

    // With module=esm, oxc reports TypeScript's CommonJS-specific syntax as errors (TS1203/TS1202)
    // instead of rewriting it.
    #[test]
    fn esm_rejects_export_assignment() {
        let errors =
            transpile("a.ts", "const x: number = 1;\nexport = x;\n", &esm_options()).unwrap_err();
        assert!(
            errors.concat().contains("Export assignment cannot be used"),
            "errors: {:?}",
            errors
        );
    }

    #[test]
    fn esm_rejects_import_equals() {
        let errors = transpile(
            "a.ts",
            "import path = require(\"node:path\");\nexport const s: string = path.sep;\n",
            &esm_options(),
        ).unwrap_err();
        assert!(
            errors.concat().contains("Import assignment cannot be used"),
            "errors: {:?}",
            errors
        );
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

    // `export {}` only marks a file as a module; CommonJS output drops it.
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
        let errors = transpile(
            "a.cts",
            "const x: number = 1;\nexport = x;\nexport {} from \"./setup.cjs\";\n",
            &commonjs_options(),
        ).unwrap_err();
        assert!(
            errors[0].contains("cannot be emitted as CommonJS"),
            "errors: {:?}",
            errors
        );
    }

    #[test]
    fn commonjs_rejects_esm_import() {
        let errors = transpile(
            "a.cts",
            "import { x } from \"./b.cjs\";\nexport = x;\n",
            &commonjs_options(),
        ).unwrap_err();
        assert!(
            errors[0].contains("cannot be emitted as CommonJS"),
            "errors: {:?}",
            errors
        );
    }

    #[test]
    fn commonjs_rejects_esm_export() {
        let errors =
            transpile("a.cts", "export const x: number = 1;\n", &commonjs_options()).unwrap_err();
        assert!(
            errors[0].contains("cannot be emitted as CommonJS"),
            "errors: {:?}",
            errors
        );
    }

    // Uses a .ts source: the parser already rejects top-level await in .cts files.
    #[test]
    fn commonjs_rejects_top_level_await() {
        let errors = transpile(
            "a.ts",
            "async function load(): Promise<number> { return 1; }\n\
             const value: number = await load();\nexport = value;\n",
            &commonjs_options(),
        ).unwrap_err();
        assert!(
            errors[0].contains("top-level await cannot be emitted as CommonJS"),
            "errors: {:?}",
            errors
        );
    }

    #[test]
    fn commonjs_rejects_top_level_for_await() {
        let errors = transpile(
            "a.ts",
            "declare const items: AsyncIterable<Promise<number>>;\n\
             for await (const item of items) { await item; }\nexport = 1;\n",
            &commonjs_options(),
        ).unwrap_err();
        assert_eq!(errors.len(), 1, "errors: {errors:?}");
        assert!(
            errors[0].contains("top-level await cannot be emitted as CommonJS"),
            "errors: {:?}",
            errors
        );
    }

    // Await inside a function is not top-level and stays valid in CommonJS.
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

    // `await using` at module scope is top-level await too.
    #[test]
    fn commonjs_rejects_top_level_await_using() {
        let errors = transpile(
            "a.ts",
            "declare const r: AsyncDisposable;\nawait using x = r;\nexport = x;\n",
            &commonjs_options(),
        ).unwrap_err();
        assert!(
            errors[0].contains("top-level await cannot be emitted as CommonJS"),
            "errors: {:?}",
            errors
        );
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
        let errors = transpile(
            "a.ts",
            "const url: string = import.meta.url;\nexport = url;\n",
            &commonjs_options(),
        ).unwrap_err();
        assert!(
            errors[0].contains("import.meta cannot be emitted as CommonJS"),
            "errors: {:?}",
            errors
        );
    }

    // Without --module the behavior is unchanged: `export =` is still rewritten (oxc does that in
    // any module mode), ESM stays ESM, and no "use strict" is added.
    #[test]
    fn preserve_keeps_esm_and_omits_use_strict() {
        let js = transpile_js("a.cts", "export const x: number = 1;\n", &js_options());
        assert!(js.contains("export const x = 1"), "js: {js}");
        assert!(!js.contains("use strict"), "js: {js}");
    }

    #[test]
    fn run_requires_module_value() {
        let err = run(args(&["--emit-js", "--module"])).unwrap_err();
        assert_eq!(err, vec!["error: --module requires a value".to_string()]);
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
        ).unwrap();
        let dts = result.dts_code.unwrap();
        assert!(
            dts.contains("export declare function add(a: number, b: number): number"),
            "dts: {dts}"
        );
        assert!(!dts.contains("return"), "dts: {dts}");
    }

    // tsc's stripInternal: /** @internal */ declarations are omitted from the dts output.
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

    // tsc's removeComments strips everything but legal comments, from the declarations too.
    #[test]
    fn remove_comments_strips_all_but_legal_comments() {
        let options = Options { remove_comments: true, ..default_options() };
        let outputs = transpile("a.ts", COMMENTED, &options).unwrap();
        let js = outputs.js_code.unwrap();
        assert!(js.contains("/*! legal */"), "js: {js}");
        assert!(!js.contains("note"), "js: {js}");
        assert!(!js.contains("Doc for x"), "js: {js}");
        let dts = outputs.dts_code.unwrap();
        assert!(!dts.contains("Doc for x"), "dts: {dts}");
    }

    // Annotations are not comments to tooling: the JSX transform's pure markers survive.
    #[test]
    fn remove_comments_keeps_pure_annotations() {
        let options = Options { remove_comments: true, ..js_options() };
        let js = transpile_js("a.tsx", "export const el = <div />;\n", &options);
        assert!(js.contains("/* @__PURE__ */"), "js: {js}");
    }

    #[test]
    fn strip_internal_omits_internal_declarations() {
        let src = "/** @internal */\nexport const secret: number = 1;\nexport const open: number = 2;\n";
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
        let elided =
            transpile("a.ts", src, &Options { emit_dts: false, ..default_options() }).unwrap();
        let js = elided.js_code.unwrap();
        assert!(!js.contains("./fx.js"), "js: {js}");

        let options = Options {
            emit_dts: false,
            only_remove_type_imports: true,
            ..default_options()
        };
        let kept = transpile("a.ts", src, &options).unwrap();
        let js = kept.js_code.unwrap();
        assert!(js.contains("import { sideEffect } from \"./fx.js\""), "js: {js}");
        assert!(!js.contains("./t.js"), "js: {js}");
    }

    #[test]
    fn transpile_reports_parse_errors() {
        transpile("bad.ts", "const = ;", &default_options()).unwrap_err();
    }

    #[test]
    fn transpile_reports_isolated_declaration_errors() {
        // Inferred return type is not allowed under isolated declarations.
        transpile(
            "a.ts",
            "export function f() { return someValue(); }\nfunction someValue() { return 1; }\n",
            &default_options(),
        ).unwrap_err();
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

    // With --use-define-for-class-fields, fields keep define semantics: uninitialized fields
    // remain as field definitions instead of being removed.
    #[test]
    fn use_define_for_class_fields_keeps_field_definitions() {
        let options = Options {
            emit_dts: false,
            use_define_for_class_fields: true,
            ..default_options()
        };
        let result = transpile(
            "a.ts",
            "export class C { declared: number; assigned = 1; }\n",
            &options,
        ).unwrap();
        let js = result.js_code.unwrap();
        assert!(js.contains("declared"), "js: {js}");
        assert!(js.contains("assigned = 1"), "js: {js}");
    }

    #[test]
    fn source_maps_emitted_when_enabled() {
        let options = Options {
            source_maps: true,
            ..default_options()
        };
        let result = transpile("a.ts", "export const x: number = 1;\n", &options).unwrap();
        let map = result.js_map.unwrap();
        assert!(map.contains("\"a.ts\""), "map: {map}");
    }

    #[test]
    fn inline_source_maps_produce_data_url() {
        let options = Options { inline_source_maps: true, ..js_options() };
        let outputs = transpile("a.ts", "export const x: number = 1;\n", &options).unwrap();
        let url = outputs.js_map_data_url.unwrap();
        assert!(url.starts_with("data:application/json;charset=utf-8;base64,"), "url: {url}");
        // The JSON form is not serialized for inline maps.
        assert!(outputs.js_map.is_none());
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
        let options = Options { source_maps: true, ..js_options() };
        let outputs = transpile("a.ts", "export const x: number = 1;\n", &options).unwrap();
        let map = outputs.js_map.unwrap();
        assert!(!map.contains("sourceRoot"), "map: {map}");
    }

    // Sources are recorded relative to --source-root-dir, since consumers resolve them against
    // the configured root rather than the map's location.
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
        assert!(map.contains("\"sourceRoot\":\"https://cdn.example/sources/\""), "map: {map}");
        assert!(map.contains("\"sources\":[\"sub/a.ts\"]"), "map: {map}");
    }

    #[test]
    fn run_rejects_source_root_dir_without_source_root() {
        let err = run(args(&["--emit-js", "--source-maps", "--source-root-dir", "src"])).unwrap_err();
        assert_eq!(err, vec!["error: --source-root-dir requires --source-root".to_string()]);
    }

    #[test]
    fn run_rejects_source_maps_with_inline_source_maps() {
        let err = run(args(&["--emit-js", "--source-maps", "--inline-source-maps"])).unwrap_err();
        assert_eq!(
            err,
            vec!["error: --source-maps and --inline-source-maps are mutually exclusive".to_string()]
        );
    }

    #[test]
    fn run_rejects_source_root_without_maps() {
        let err = run(args(&["--emit-js", "--source-root", "/src"])).unwrap_err();
        assert!(err[0].starts_with("error: --source-root requires"), "{err:?}");
    }

    #[test]
    fn run_requires_source_root_value() {
        let err = run(args(&["--emit-js", "--source-root"])).unwrap_err();
        assert_eq!(err, vec!["error: --source-root requires a value".to_string()]);
    }

    #[test]
    fn declaration_maps_emitted_when_enabled() {
        let options = Options { declaration_maps: true, ..default_options() };
        let outputs = transpile("a.ts", "export const x: number = 1;\n", &options).unwrap();
        assert!(outputs.js_map.is_none());
        let map = outputs.dts_map.unwrap();
        assert!(map.contains("\"a.ts\""), "map: {map}");
        assert!(!map.contains("\"mappings\":\"\""), "map: {map}");
    }

    #[test]
    fn declaration_maps_not_emitted_by_default() {
        let outputs = transpile("a.ts", "export const x: number = 1;\n", &default_options()).unwrap();
        assert!(outputs.dts_map.is_none());
    }

    #[test]
    fn run_rejects_declaration_maps_without_emit_dts() {
        let err = run(args(&["--emit-js", "--declaration-maps"])).unwrap_err();
        assert_eq!(err, vec!["error: --declaration-maps requires --emit-dts".to_string()]);
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
        let js = transpile_js("a.tsx", "export const el: object = <div id={1} />;\n", &js_options());
        assert!(js.contains("react/jsx-runtime"), "js: {js}");
        assert!(js.contains("_jsx("), "js: {js}");
        assert!(!js.contains(": object"), "js: {js}");
    }

    #[test]
    fn transpile_transforms_jsx_in_plain_jsx_file() {
        let js = transpile_js("a.jsx", "export const el = <div id={1} />;\n", &js_options());
        assert!(js.contains("_jsx("), "js: {js}");
    }

    // The classic runtime compiles JSX to React.createElement calls and imports nothing;
    // providing React is the caller's concern, as with tsc's jsx=react.
    #[test]
    fn jsx_classic_uses_create_element() {
        let options = Options { jsx: JsxRuntime::Classic, ..js_options() };
        let js = transpile_js("a.tsx", "export const el: object = <div id={1} />;\n", &options);
        assert!(js.contains("React.createElement"), "js: {js}");
        assert!(!js.contains("react/jsx-runtime"), "js: {js}");
        assert!(!js.contains("<div"), "js: {js}");
    }

    // jsxImportSource: the automatic runtime imports from the given module.
    #[test]
    fn jsx_import_source_changes_runtime_module() {
        let options = Options {
            emit_dts: false,
            jsx_import_source: Some("preact".to_string()),
            ..default_options()
        };
        let result = transpile("a.tsx", "export const el = <div />;\n", &options).unwrap();
        let js = result.js_code.unwrap();
        assert!(js.contains("\"preact/jsx-runtime\""), "js: {js}");
        assert!(!js.contains("\"react/jsx-runtime\""), "js: {js}");
    }

    // jsxFactory/jsxFragmentFactory: the classic runtime uses the given pragma, and the pragma's
    // import survives type stripping.
    #[test]
    fn jsx_pragma_changes_classic_factory() {
        let options = Options {
            emit_dts: false,
            jsx: JsxRuntime::Classic,
            jsx_pragma: Some("h".to_string()),
            jsx_pragma_frag: Some("Fragment".to_string()),
            ..default_options()
        };
        let result = transpile(
            "a.tsx",
            "import { h, Fragment } from \"preact\";\nexport const el = <div><span /></div>;\nexport const frag = <></>;\n",
            &options,
        ).unwrap();
        let js = result.js_code.unwrap();
        assert!(js.contains("h(\"div\""), "js: {js}");
        assert!(js.contains("h(Fragment"), "js: {js}");
        assert!(js.contains("import { h, Fragment } from \"preact\""), "js: {js}");
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
        assert_eq!(err, vec!["error: --jsx-import-source requires --jsx automatic".to_string()]);
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
    fn run_requires_jsx_option_values() {
        for flag in ["--jsx-import-source", "--jsx-pragma", "--jsx-pragma-frag"] {
            let err = run(args(&["--emit-js", flag])).unwrap_err();
            assert_eq!(err, vec![format!("error: {flag} requires a value")]);
        }
    }

    #[test]
    fn run_requires_jsx_value() {
        let err = run(args(&["--emit-js", "--jsx"])).unwrap_err();
        assert_eq!(err, vec!["error: --jsx requires a value".to_string()]);
    }

    #[test]
    fn run_rejects_unsupported_jsx() {
        let err = run(args(&["--emit-js", "--jsx", "react-jsx"])).unwrap_err();
        assert!(err[0].contains("unsupported --jsx \"react-jsx\""), "{err:?}");
        let err = run(args(&["--emit-js", "--jsx", "preserve"])).unwrap_err();
        assert!(err[0].contains("unsupported --jsx \"preserve\""), "{err:?}");
    }

    // Specifiers are emitted verbatim, like tsc: no filesystem-based resolution of extensionless
    // or directory imports.
    #[test]
    fn transpile_emits_specifiers_verbatim() {
        let dir = test_dir("transpile_verbatim");
        fs::write(dir.join("b.ts"), "").unwrap();
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/index.ts"), "").unwrap();
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
        let options = Options { rewrite_extensions: true, ..js_options() };
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
        fs::write(dir.join("b.ts"), "").unwrap();
        for rewrite_extensions in [false, true] {
            let options = Options { rewrite_extensions, ..js_options() };
            let src = dir.join("a.ts");
            let js = transpile_js(src.to_str().unwrap(), "export const p = import(\"./b\");\n", &options);
            assert!(js.contains("import(\"./b\")"), "js: {js}");
        }
    }

    #[test]
    fn transpile_leaves_non_literal_dynamic_import() {
        let dir = test_dir("transpile_dynamic_import_non_literal");
        let options = Options { rewrite_extensions: true, ..js_options() };
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
        let options = Options { rewrite_extensions: true, ..js_options() };
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

    // oxc rewrites any slash-containing specifier, bare package paths included, unlike tsc.
    #[test]
    fn transpile_rewrites_bare_specifier_with_slash() {
        let options = Options { rewrite_extensions: true, ..js_options() };
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
        Options { experimental_decorators: true, ..js_options() }
    }

    // Without --experimental-decorators oxc has no transform to apply: decorators are emitted
    // as written, for a runtime or bundler that understands the standard proposal.
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
        assert!(js.contains("@oxc-project/runtime/helpers/decorate"), "js: {js}");
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
        let options = Options { emit_decorator_metadata: true, ..decorator_options() };
        let js = transpile_js("a.ts", DECORATED, &options);
        assert!(js.contains("_decorateMetadata(\"design:type\", Function)"), "js: {js}");
        assert!(js.contains("_decorateMetadata(\"design:paramtypes\", [Object])"), "js: {js}");
        assert!(js.contains("_decorateMetadata(\"design:returntype\", void 0)"), "js: {js}");
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
        assert!(js.contains("_decorateMetadata(\"design:paramtypes\", [String])"), "js: {js}");
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
    fn transpile_module_variant_sources() {
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
                vec![format!("error: --cpus must be a positive integer, got \"{value}\"")]
            );
        }
    }

    #[test]
    fn run_requires_cpu_count_value() {
        let err = run(args(&["--emit-js", "--cpus"])).unwrap_err();
        assert_eq!(err, vec!["error: --cpus requires a positive integer".to_string()]);
    }

    #[test]
    fn run_requires_manifest_path() {
        let err = run(args(&["--emit-js", "--manifest"])).unwrap_err();
        assert_eq!(err, vec!["error: --manifest requires a file path".to_string()]);
    }

    #[test]
    fn run_reports_unreadable_manifest() {
        let dir = test_dir("run_missing_manifest");
        let manifest = dir.join("missing.txt");
        let err = run(args(&["--emit-js", "--manifest", manifest.to_str().unwrap()])).unwrap_err();
        assert!(err[0].starts_with("error: cannot read manifest"), "{err:?}");
    }

    #[test]
    fn run_rejects_misaligned_entries() {
        let dir = test_dir("run_misaligned");
        let src = dir.join("a.ts");
        fs::write(&src, "export const x = 1;\n").unwrap();
        // --emit-js and --emit-dts expect 3 lines per entry; give 2.
        let err = run(args(&[
            "--emit-js",
            "--emit-dts",
            src.to_str().unwrap(),
            dir.join("a.js").to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(err[0].contains("expected entries of 3 lines"), "{err:?}");
    }

    #[test]
    fn run_transpiles_manifest_entries() {
        let dir = test_dir("run_manifest");
        let src = dir.join("a.ts");
        fs::write(&src, "export const x: number = 1;\n").unwrap();
        let js_out = dir.join("out/a.js");
        let dts_out = dir.join("out/a.d.ts");
        let manifest = dir.join("manifest.txt");
        fs::write(
            &manifest,
            format!(
                "{}\n{}\n{}\n",
                src.display(),
                js_out.display(),
                dts_out.display()
            ),
        )
        .unwrap();
        run(args(&[
            "--emit-js",
            "--emit-dts",
            "--manifest",
            manifest.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(read(&js_out).contains("export const x = 1"));
        assert!(read(&dts_out).contains("declare const x: number"));
    }

    #[test]
    fn run_transpiles_positional_entries() {
        let dir = test_dir("run_positional");
        let src = dir.join("a.ts");
        fs::write(&src, "export const x: number = 1;\n").unwrap();
        let js_out = dir.join("a.out.js");
        run(args(&[
            "--emit-js",
            src.to_str().unwrap(),
            js_out.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(read(&js_out).contains("export const x = 1"));
    }

    #[test]
    fn run_accepts_cpu_count() {
        let dir = test_dir("run_parallel");
        let first = dir.join("first.ts");
        let second = dir.join("second.ts");
        fs::write(&first, "export const first: number = 1;\n").unwrap();
        fs::write(&second, "export const second: number = 2;\n").unwrap();
        let first_out = dir.join("out/first.js");
        let second_out = dir.join("out/second.js");
        run(args(&[
            "--emit-js",
            "--cpus",
            "2",
            first.to_str().unwrap(),
            first_out.to_str().unwrap(),
            second.to_str().unwrap(),
            second_out.to_str().unwrap(),
        ]))
        .unwrap();
        assert!(read(&first_out).contains("export const first = 1"));
        assert!(read(&second_out).contains("export const second = 2"));
    }

    #[test]
    fn run_reports_parallel_errors_in_manifest_order() {
        let dir = test_dir("run_parallel_error_order");
        let mut cli = vec!["--emit-js".to_string(), "--cpus".to_string(), "2".to_string()];
        for i in 0..6 {
            let src = dir.join(format!("{i}.ts"));
            fs::write(&src, "export const x: number = ;\n").unwrap();
            cli.push(src.to_str().unwrap().to_string());
            cli.push(dir.join(format!("out/{i}.js")).to_str().unwrap().to_string());
        }
        let err = run(cli.into_iter()).unwrap_err();
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
        let src = dir.join("a.js");
        fs::write(&src, "export const x = 1;\n").unwrap();
        let js_out = dir.join("out/a.js");
        let manifest = dir.join("manifest.txt");
        // Plain JS entry: empty declaration output line means no dts emitted.
        fs::write(
            &manifest,
            format!("{}\n{}\n\n", src.display(), js_out.display()),
        )
        .unwrap();
        run(args(&[
            "--emit-js",
            "--emit-dts",
            "--manifest",
            manifest.to_str().unwrap(),
        ]))
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
            let src = dir.join(name);
            fs::write(&src, "export declare const x: number;\n").unwrap();
            let dts_out = dir.join("out").join(name);
            let manifest = dir.join("manifest.txt");
            fs::write(
                &manifest,
                format!("{}\n\n{}\n", src.display(), dts_out.display()),
            )
            .unwrap();
            let err = run(args(&[
                "--emit-js",
                "--emit-dts",
                "--manifest",
                manifest.to_str().unwrap(),
            ]))
            .unwrap_err();
            assert!(err[0].contains("produces no outputs"), "{err:?}");
            assert!(!dts_out.exists());
        }
    }

    #[test]
    fn run_appends_source_mapping_url_and_writes_map() {
        let dir = test_dir("run_source_maps");
        let src = dir.join("a.ts");
        fs::write(&src, "export const x: number = 1;\n").unwrap();
        let js_out = dir.join("out/a.js");
        run(args(&[
            "--emit-js",
            "--source-maps",
            src.to_str().unwrap(),
            js_out.to_str().unwrap(),
        ]))
        .unwrap();
        let js = read(&js_out);
        assert!(js.ends_with("= 1;\n//# sourceMappingURL=a.js.map"), "js: {js}");
        // The source is recorded relative to the map's directory.
        let map = read(&dir.join("out/a.js.map"));
        assert!(map.contains("\"sources\":[\"../a.ts\"]"), "map: {map}");
    }

    // Inline maps embed the map in the JS itself: no .js.map file is written.
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
        let src = dir.join("a.ts");
        fs::write(&src, "export const x: number = 1;\n").unwrap();
        fs::create_dir_all(dir.join("types")).unwrap();
        let js_out = dir.join("out/a.js");
        let dts_out = dir.join("types/a.d.ts");
        run(args(&[
            "--emit-js",
            "--emit-dts",
            "--declaration-maps",
            src.to_str().unwrap(),
            js_out.to_str().unwrap(),
            dts_out.to_str().unwrap(),
        ]))
        .unwrap();
        // Only the declaration gets a map: --source-maps was not given.
        assert!(!dir.join("out/a.js.map").exists());
        assert!(!read(&js_out).contains("sourceMappingURL"));
        let dts = read(&dts_out);
        assert!(dts.ends_with("//# sourceMappingURL=a.d.ts.map"), "dts: {dts}");
        let map = read(&dir.join("types/a.d.ts.map"));
        assert!(map.contains("\"sources\":[\"../a.ts\"]"), "map: {map}");
    }

    #[test]
    fn relative_to_walks_up_to_common_ancestor() {
        assert_eq!(
            relative_to(Path::new("pkg/src/a.ts"), Path::new("bazel-out/bin/pkg/dist")),
            PathBuf::from("../../../../pkg/src/a.ts")
        );
        assert_eq!(
            relative_to(Path::new("pkg/src/a.ts"), Path::new("pkg/src")),
            PathBuf::from("a.ts")
        );
        assert_eq!(
            relative_to(Path::new("pkg/src/a.ts"), Path::new("pkg/dist/sub")),
            PathBuf::from("../../src/a.ts")
        );
    }

    #[test]
    fn run_writes_successful_entries_while_aggregating_errors() {
        let dir = test_dir("run_error_aggregation");
        let bad1 = dir.join("bad1.ts");
        let bad2 = dir.join("bad2.ts");
        let good = dir.join("good.ts");
        fs::write(&bad1, "const = ;").unwrap();
        fs::write(&bad2, "const = ;").unwrap();
        fs::write(&good, "export const x = 1;\n").unwrap();
        let good_out = dir.join("out/good.js");
        let err = run(args(&[
            "--emit-js",
            bad1.to_str().unwrap(),
            dir.join("out/bad1.js").to_str().unwrap(),
            bad2.to_str().unwrap(),
            dir.join("out/bad2.js").to_str().unwrap(),
            good.to_str().unwrap(),
            good_out.to_str().unwrap(),
        ]))
        .unwrap_err();
        // Both failing entries are reported, while successful entries are written immediately.
        assert!(err.len() >= 2, "errors: {err:?}");
        assert!(good_out.exists());
    }

    #[test]
    fn run_writes_js_maps_and_declarations_before_later_errors() {
        let dir = test_dir("run_streams_all_output_kinds");
        let good = dir.join("good.ts");
        let bad = dir.join("bad.ts");
        fs::write(&good, "export const x: number = 1;\n").unwrap();
        fs::write(&bad, "const = ;").unwrap();
        let js_out = dir.join("out/good.js");
        let dts_out = dir.join("out/good.d.ts");
        let err = run(args(&[
            "--emit-js",
            "--emit-dts",
            "--source-maps",
            good.to_str().unwrap(),
            js_out.to_str().unwrap(),
            dts_out.to_str().unwrap(),
            bad.to_str().unwrap(),
            dir.join("out/bad.js").to_str().unwrap(),
            dir.join("out/bad.d.ts").to_str().unwrap(),
        ]))
        .unwrap_err();

        assert!(!err.is_empty(), "errors: {err:?}");
        assert!(js_out.exists());
        assert!(js_out.with_extension("js.map").exists());
        assert!(dts_out.exists());
    }

    // A write failure is reported as an error like any other, not a panic.
    #[test]
    fn run_reports_unwritable_output() {
        let dir = test_dir("run_unwritable_output");
        let src = dir.join("a.ts");
        fs::write(&src, "export const x: number = 1;\n").unwrap();
        // A plain file where the output's parent directory should be.
        let blocker = dir.join("blocker");
        fs::write(&blocker, "").unwrap();
        let out = blocker.join("a.js");
        let err =
            run(args(&["--emit-js", src.to_str().unwrap(), out.to_str().unwrap()])).unwrap_err();
        assert!(err[0].starts_with("error: cannot write"), "{err:?}");
    }

    // Output parent directories are Bazel's job; a missing one is an error, not created.
    #[test]
    fn run_does_not_create_output_directories() {
        let dir = test_dir("run_missing_output_dir");
        let src = dir.join("a.ts");
        fs::write(&src, "export const x: number = 1;\n").unwrap();
        let out = dir.join("missing/a.js");
        let err =
            run(args(&["--emit-js", src.to_str().unwrap(), out.to_str().unwrap()])).unwrap_err();
        assert!(err[0].starts_with("error: cannot write"), "{err:?}");
        assert!(!dir.join("missing").exists());
    }

    // An unreadable source is reported alongside other entries' errors instead of aborting the pass.
    #[test]
    fn run_aggregates_read_errors_across_entries() {
        let dir = test_dir("run_read_error_aggregation");
        let missing = dir.join("missing.ts");
        let bad = dir.join("bad.ts");
        fs::write(&bad, "const = ;").unwrap();
        let err = run(args(&[
            "--emit-js",
            missing.to_str().unwrap(),
            dir.join("out/missing.js").to_str().unwrap(),
            bad.to_str().unwrap(),
            dir.join("out/bad.js").to_str().unwrap(),
        ]))
        .unwrap_err();
        assert!(err.len() >= 2, "errors: {err:?}");
        assert!(err[0].contains("cannot read"), "errors: {err:?}");
    }
}
