use oxc::allocator::Allocator;
use oxc::ast::ast::{
    ArrowFunctionExpression, AwaitExpression, ForOfStatement, Function, Program, Statement,
};
use oxc::ast_visit::{Visit, walk};
use oxc::syntax::scope::ScopeFlags;
use oxc::codegen::{Codegen, CodegenOptions};
use oxc::diagnostics::{GraphicalReportHandler, GraphicalTheme, NamedSource, OxcDiagnostic};
use oxc::isolated_declarations::{
    IsolatedDeclarations, IsolatedDeclarationsOptions as OxcIsolatedDeclarationsOptions,
};
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::{GetSpan, SourceType, Span};
use oxc::syntax::module_record::ModuleRecord;
use oxc::transformer::{
    CompilerAssumptions, EnvOptions, HelperLoaderOptions, JsxOptions, JsxRuntime, Module,
    RewriteExtensionsMode, TransformOptions, Transformer, TypeScriptOptions,
};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone)]
struct Options {
    emit_js: bool,
    emit_dts: bool,
    source_maps: bool,
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
    // Directories forming one virtual directory for resolving relative specifiers, like tsc's
    // rootDirs. Empty means only the source's own directory is searched.
    root_dirs: Vec<PathBuf>,
}

struct Entry {
    src: String,
    js_out: Option<String>,
    dts_out: Option<String>,
}

struct Output {
    path: String,
    code: String,
    map: Option<String>,
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

fn run(mut args: impl Iterator<Item = String>) -> Result<(), Vec<String>> {
    let mut options = Options {
        emit_js: false,
        emit_dts: false,
        source_maps: false,
        jsx: JsxRuntime::Automatic,
        rewrite_extensions: false,
        env: None,
        helpers_module: None,
        module: Module::Preserve,
        root_dirs: Vec::new(),
    };
    let mut manifest_path: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--emit-js" => options.emit_js = true,
            "--emit-dts" => options.emit_dts = true,
            "--source-maps" => options.source_maps = true,
            "--jsx" => {
                let value = flag_value(&mut args, &arg, "a value")?;
                options.jsx = match value.as_str() {
                    "automatic" => JsxRuntime::Automatic,
                    "classic" => JsxRuntime::Classic,
                    _ => {
                        return Err(vec![format!(
                            "error: unsupported --jsx \"{value}\": expected \"automatic\" or \"classic\""
                        )]);
                    }
                };
            }
            "--rewrite-extensions" => options.rewrite_extensions = true,
            "--target" => {
                let target = flag_value(&mut args, &arg, "a value")?;
                options.env = Some(
                    EnvOptions::from_target(&target)
                        .map_err(|e| vec![format!("error: invalid --target \"{target}\": {e}")])?,
                );
            }
            "--helpers-module" => {
                options.helpers_module = Some(flag_value(&mut args, &arg, "a value")?);
            }
            "--module" => {
                let value = flag_value(&mut args, &arg, "a value")?;
                options.module = match value.as_str() {
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
            "--root-dirs" => {
                options.root_dirs.push(PathBuf::from(flag_value(&mut args, &arg, "a path")?));
            }
            "--manifest" => {
                manifest_path = Some(flag_value(&mut args, &arg, "a file path")?);
            }
            _ => positional.push(arg),
        }
    }

    if !options.emit_js && !options.emit_dts {
        return Err(vec![
            "error: at least one of --emit-js or --emit-dts is required".to_string(),
        ]);
    }

    // Each manifest entry is the source path followed by the JS output path (when --emit-js) and
    // the declaration output path (when --emit-dts). An empty output path skips that output for
    // the entry (e.g. no declarations for plain JS sources).
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

    let entries: Vec<Entry> = lines
        .chunks(entry_width)
        .map(|chunk| {
            let mut outs = chunk[1..].iter();
            Entry {
                src: chunk[0].clone(),
                js_out: options
                    .emit_js
                    .then(|| outs.next().unwrap().clone())
                    .filter(|out| !out.is_empty()),
                dts_out: options
                    .emit_dts
                    .then(|| outs.next().unwrap().clone())
                    .filter(|out| !out.is_empty()),
            }
        })
        .collect();

    let mut all_errors: Vec<String> = Vec::new();
    let mut outputs: Vec<Output> = Vec::new();

    for entry in &entries {
        let content = fs::read_to_string(&entry.src)
            .map_err(|e| vec![format!("error: cannot read {}: {e}", entry.src)])?;

        // Declaration files pass through unchanged and have no JS output.
        if is_declaration_file(&entry.src) {
            if let Some(dts_out) = &entry.dts_out {
                outputs.push(Output { path: dts_out.clone(), code: content, map: None });
            }
            continue;
        }

        let entry_options = Options {
            emit_js: entry.js_out.is_some(),
            emit_dts: entry.dts_out.is_some(),
            ..options.clone()
        };
        let result = transpile(&entry.src, &content, &entry_options);
        if !result.errors.is_empty() {
            all_errors.extend(result.errors);
            continue;
        }
        if let (Some(js_out), Some(code)) = (&entry.js_out, result.js_code) {
            outputs.push(Output { path: js_out.clone(), code, map: result.js_map });
        }
        if let (Some(dts_out), Some(code)) = (&entry.dts_out, result.dts_code) {
            outputs.push(Output { path: dts_out.clone(), code, map: None });
        }
    }

    if !all_errors.is_empty() {
        return Err(all_errors);
    }

    for output in outputs {
        let out = Path::new(&output.path);
        fs::create_dir_all(out.parent().unwrap()).unwrap();
        if let Some(map) = output.map {
            let map_basename = out.file_name().unwrap().to_string_lossy();
            let code = format!("{}\n//# sourceMappingURL={map_basename}.map", output.code);
            fs::write(out, code).unwrap();
            fs::write(format!("{}.map", output.path), map).unwrap();
        } else {
            fs::write(out, output.code).unwrap();
        }
    }

    Ok(())
}

// Matches the Bazel rule's _is_declaration: all three declaration extensions, so a .d.mts/.d.cts
// input passes through instead of hitting the transpile path (isolated declarations would error
// on a declaration file).
fn is_declaration_file(path: &str) -> bool {
    path.ends_with(".d.ts") || path.ends_with(".d.mts") || path.ends_with(".d.cts")
}

#[derive(Default)]
struct TranspileResult {
    js_code: Option<String>,
    js_map: Option<String>,
    dts_code: Option<String>,
    errors: Vec<String>,
}

fn build_transform_options(options: &Options) -> TransformOptions {
    TransformOptions {
        // With remove_class_fields_without_initializer below, matches tsc's useDefineForClassFields=false:
        // fields are assigned with `=` rather than Object.defineProperty, and fields without an
        // initializer are removed rather than set to undefined.
        assumptions: CompilerAssumptions {
            set_public_class_fields: true,
            ..Default::default()
        },
        typescript: TypeScriptOptions {
            remove_class_fields_without_initializer: true,
            // Like tsc's rewriteRelativeImportExtensions, but Babel semantics: any slash-containing
            // .ts/.tsx/.mts/.cts specifier is rewritten, and .tsx always maps to .js (never .jsx).
            rewrite_import_extensions: options
                .rewrite_extensions
                .then_some(RewriteExtensionsMode::Rewrite),
            ..Default::default()
        },
        jsx: JsxOptions {
            runtime: options.jsx,
            ..JsxOptions::default()
        },
        env: {
            let mut env = options.env.clone().unwrap_or_default();
            env.module = options.module;
            env
        },
        // Helpers are imported like tsc's importHelpers (oxc's only implemented mode);
        // --helpers-module redirects the imports away from the default @oxc-project/runtime.
        helper_loader: HelperLoaderOptions {
            module_name: match &options.helpers_module {
                Some(name) => name.clone().into(),
                None => HelperLoaderOptions::default().module_name,
            },
            ..Default::default()
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
        }
        walk::walk_for_of_statement(self, stmt);
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
    let source = NamedSource::new(filename, source_text.to_string());
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
// first, before the transformer mutates the program.
fn transpile(filename: &str, source_text: &str, options: &Options) -> TranspileResult {
    // Every source is parsed as TypeScript, including plain .js/.jsx: one pipeline for all inputs.
    let source_type = SourceType::from_path(filename)
        .unwrap_or_default()
        .with_typescript(true);

    let allocator = Allocator::default();
    let mut parser_ret = Parser::new(&allocator, source_text, source_type).parse();

    let mut result = TranspileResult::default();

    if !parser_ret.diagnostics.is_empty() {
        result.errors = render_errors(filename, source_text, parser_ret.diagnostics);
        return result;
    }

    if options.emit_dts {
        let decl_ret = IsolatedDeclarations::new(
            &allocator,
            OxcIsolatedDeclarationsOptions {
                strip_internal: false,
            },
        )
        .build(&parser_ret.program);

        if !decl_ret.diagnostics.is_empty() {
            result.errors = render_errors(filename, source_text, decl_ret.diagnostics);
            return result;
        }

        result.dts_code = Some(Codegen::new().build(&decl_ret.program).code);
    }

    if options.emit_js {
        let semantic_ret = SemanticBuilder::new().build(&parser_ret.program);
        let semantic_diagnostics = semantic_ret.diagnostics;
        let scoping = semantic_ret.semantic.into_scoping();

        let transform_options = build_transform_options(options);

        let transformer_ret = Transformer::new(&allocator, Path::new(filename), &transform_options)
            .build_with_scoping(scoping, &mut parser_ret.program);

        let diagnostics: Vec<_> = semantic_diagnostics
            .into_iter()
            .chain(transformer_ret.diagnostics)
            .collect();
        if !diagnostics.is_empty() {
            result.errors = render_errors(filename, source_text, diagnostics);
            return result;
        }

        if options.module.is_commonjs() {
            // An empty `export {}` — hand-written, or appended by the transformer after erasing a
            // file's only module syntax — is dropped rather than rejected, matching tsc.
            parser_ret.program.body.retain(|stmt| !is_empty_export(stmt));
            let diagnostics =
                commonjs_diagnostics(&parser_ret.program, &parser_ret.module_record);
            if !diagnostics.is_empty() {
                result.errors = render_errors(filename, source_text, diagnostics);
                return result;
            }
        }

        resolve_relative_specifiers(
            &mut parser_ret.program,
            &allocator,
            Path::new(filename),
            &options.root_dirs,
        );

        let mut codegen = Codegen::new();
        if options.source_maps {
            codegen = codegen.with_options(CodegenOptions {
                source_map_path: Some(PathBuf::from(filename)),
                ..Default::default()
            });
        }
        let codegen_ret = codegen.build(&parser_ret.program);

        result.js_code = Some(codegen_ret.code);
        result.js_map = codegen_ret.map.map(|m| m.to_json_string());
    }

    result
}

// Node's ESM loader requires fully-specified relative specifiers: no directory imports and no
// extensionless imports. TS's "bundler" moduleResolution allows both (omitting the extension, or
// importing a directory resolved via its index file), so those are resolved here against the
// source files on disk.
//
// Extensioned specifiers are left alone: rewriting TypeScript extensions is the transformer's
// `rewrite_import_extensions` option. Static imports are top-level only, so no full AST walk is needed.
fn resolve_relative_specifiers<'a>(
    program: &mut Program<'a>,
    allocator: &'a Allocator,
    filename: &Path,
    root_dirs: &[PathBuf],
) {
    let base_dir = filename.parent().unwrap_or_else(|| Path::new(""));


    for stmt in program.body.iter_mut() {
        let source = match stmt {
            Statement::ImportDeclaration(decl) => Some(&mut decl.source),
            Statement::ExportFromDeclaration(decl) => Some(&mut decl.source),
            Statement::ExportAllDeclaration(decl) => Some(&mut decl.source),
            _ => None,
        };

        if let Some(source) = source
            && let Some(resolved) =
                resolve_specifier(base_dir, source.value.as_str(), root_dirs)
        {
            source.value = allocator.alloc_str(&resolved).into();
            source.raw = None;
        }
    }
}

// TypeScript sources first: with both foo.ts and foo.js present, tsc resolves "./foo" to foo.ts.
const RESOLVABLE_EXTS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

fn js_ext_for(src_ext: &str) -> &'static str {
    match src_ext {
        "mts" | "mjs" => "mjs",
        "cts" | "cjs" => "cjs",
        _ => "js",
    }
}

// Declaration-only targets resolve like tsc: importing "./gen" where only gen.d.ts exists emits
// "./gen.js", assuming the implementation exists at runtime.
const DECLARATION_SUFFIXES: &[(&str, &str)] =
    &[(".d.ts", "js"), (".d.mts", "mjs"), (".d.cts", "cjs")];

// Lexically fold "." and ".." components so root prefixes match without filesystem access.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// Paths to probe for a relative specifier: the normalized import target first, then — when it lies
// under a root_dir, taking the longest matching root like tsc — the same relative location under each
// other root, mirroring how tsc's rootDirs overlays the roots into one virtual directory. Matching the
// target rather than the importer's directory keeps "../" specifiers that leave an overlapping root resolvable.
fn candidate_targets(base_dir: &Path, specifier: &str, root_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let target = normalize(&base_dir.join(specifier));
    let mut targets = vec![target.clone()];

    let longest = root_dirs
        .iter()
        .filter_map(|root| target.strip_prefix(root).ok().map(|rel| (root, rel.to_path_buf())))
        .max_by_key(|(root, _)| root.components().count());

    if let Some((longest_root, rel)) = longest {
        for other in root_dirs {
            if other != longest_root {
                let candidate = other.join(&rel);
                if !targets.contains(&candidate) {
                    targets.push(candidate);
                }
            }
        }
    }
    targets
}

fn resolve_specifier(base_dir: &Path, specifier: &str, root_dirs: &[PathBuf]) -> Option<String> {
    if !specifier.starts_with("./") && !specifier.starts_with("../") {
        return None;
    }

    // Already fully specified; extension rewriting is the transformer's job.
    if Path::new(specifier).extension().is_some() {
        return None;
    }

    // Each candidate target is resolved completely (file, declaration, then index file) before moving
    // to the next, matching tsc: a directory's own index file wins over a file in a later root.
    for target in candidate_targets(base_dir, specifier, root_dirs) {
        if let Some(js_ext) = probe(&target) {
            return Some(format!("{specifier}.{js_ext}"));
        }
        if let Some(js_ext) = probe(&target.join("index")) {
            return Some(format!("{specifier}/index.{js_ext}"));
        }
    }

    None
}

// The emitted JS extension for a source or declaration file existing at `target` with any
// resolvable extension appended, or None when no such file exists.
fn probe(target: &Path) -> Option<&'static str> {
    for &ext in RESOLVABLE_EXTS {
        if target.with_extension(ext).is_file() {
            return Some(js_ext_for(ext));
        }
    }

    for &(suffix, js_ext) in DECLARATION_SUFFIXES {
        let mut decl = target.to_path_buf().into_os_string();
        decl.push(suffix);
        if Path::new(&decl).is_file() {
            return Some(js_ext);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_options() -> Options {
        Options {
            emit_js: true,
            emit_dts: true,
            source_maps: false,
            jsx: JsxRuntime::Automatic,
            rewrite_extensions: false,
            env: None,
            helpers_module: None,
            module: Module::Preserve,
            root_dirs: Vec::new(),
        }
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
        let result = transpile(filename, source_text, options);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        result.js_code.unwrap()
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
        let err = run(args(&["--emit-js", "--target", "es1999"])).unwrap_err();
        assert!(err[0].starts_with("error: invalid --target \"es1999\""), "{err:?}");
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
        let result = transpile("a.ts", "const x: number = 1;\nexport = x;\n", &esm_options());
        assert!(!result.errors.is_empty());
        assert!(
            result.errors.concat().contains("Export assignment cannot be used"),
            "errors: {:?}",
            result.errors
        );
        assert!(result.js_code.is_none());
    }

    #[test]
    fn esm_rejects_import_equals() {
        let result = transpile(
            "a.ts",
            "import path = require(\"node:path\");\nexport const s: string = path.sep;\n",
            &esm_options(),
        );
        assert!(!result.errors.is_empty());
        assert!(
            result.errors.concat().contains("Import assignment cannot be used"),
            "errors: {:?}",
            result.errors
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
        let result = transpile(
            "a.cts",
            "const x: number = 1;\nexport = x;\nexport {} from \"./setup.cjs\";\n",
            &commonjs_options(),
        );
        assert!(!result.errors.is_empty());
        assert!(
            result.errors[0].contains("cannot be emitted as CommonJS"),
            "errors: {:?}",
            result.errors
        );
        assert!(result.js_code.is_none());
    }

    #[test]
    fn commonjs_rejects_esm_import() {
        let result = transpile(
            "a.cts",
            "import { x } from \"./b.cjs\";\nexport = x;\n",
            &commonjs_options(),
        );
        assert!(!result.errors.is_empty());
        assert!(
            result.errors[0].contains("cannot be emitted as CommonJS"),
            "errors: {:?}",
            result.errors
        );
        assert!(result.js_code.is_none());
    }

    #[test]
    fn commonjs_rejects_esm_export() {
        let result = transpile("a.cts", "export const x: number = 1;\n", &commonjs_options());
        assert!(!result.errors.is_empty());
        assert!(
            result.errors[0].contains("cannot be emitted as CommonJS"),
            "errors: {:?}",
            result.errors
        );
    }

    // Uses a .ts source: the parser already rejects top-level await in .cts files.
    #[test]
    fn commonjs_rejects_top_level_await() {
        let result = transpile(
            "a.ts",
            "async function load(): Promise<number> { return 1; }\n\
             const value: number = await load();\nexport = value;\n",
            &commonjs_options(),
        );
        assert!(!result.errors.is_empty());
        assert!(
            result.errors[0].contains("top-level await cannot be emitted as CommonJS"),
            "errors: {:?}",
            result.errors
        );
        assert!(result.js_code.is_none());
    }

    #[test]
    fn commonjs_rejects_top_level_for_await() {
        let result = transpile(
            "a.ts",
            "declare const items: AsyncIterable<number>;\nfor await (const item of items) {\n}\n\
             export = 1;\n",
            &commonjs_options(),
        );
        assert!(!result.errors.is_empty());
        assert!(
            result.errors[0].contains("top-level await cannot be emitted as CommonJS"),
            "errors: {:?}",
            result.errors
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

    // import.meta is an expression, not a module-declaration statement, so it is caught via the module
    // record; Node rejects it in CommonJS files. (.ts source: the parser already rejects it in .cts.)
    #[test]
    fn commonjs_rejects_import_meta() {
        let result = transpile(
            "a.ts",
            "const url: string = import.meta.url;\nexport = url;\n",
            &commonjs_options(),
        );
        assert!(!result.errors.is_empty());
        assert!(
            result.errors[0].contains("import.meta cannot be emitted as CommonJS"),
            "errors: {:?}",
            result.errors
        );
        assert!(result.js_code.is_none());
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
    fn js_ext_mapping() {
        assert_eq!(js_ext_for("ts"), "js");
        assert_eq!(js_ext_for("tsx"), "js");
        assert_eq!(js_ext_for("mts"), "mjs");
        assert_eq!(js_ext_for("cts"), "cjs");
        assert_eq!(js_ext_for("js"), "js");
        assert_eq!(js_ext_for("jsx"), "js");
        assert_eq!(js_ext_for("mjs"), "mjs");
        assert_eq!(js_ext_for("cjs"), "cjs");
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

    #[test]
    fn transpile_emits_declarations() {
        let result = transpile(
            "a.ts",
            "export function add(a: number, b: number): number { return a + b; }\n",
            &default_options(),
        );
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let dts = result.dts_code.unwrap();
        assert!(
            dts.contains("export declare function add(a: number, b: number): number"),
            "dts: {dts}"
        );
        assert!(!dts.contains("return"), "dts: {dts}");
    }

    #[test]
    fn transpile_reports_parse_errors() {
        let result = transpile("bad.ts", "const = ;", &default_options());
        assert!(!result.errors.is_empty());
        assert!(result.js_code.is_none());
        assert!(result.dts_code.is_none());
    }

    #[test]
    fn transpile_reports_isolated_declaration_errors() {
        // Inferred return type is not allowed under isolated declarations.
        let result = transpile(
            "a.ts",
            "export function f() { return someValue(); }\nfunction someValue() { return 1; }\n",
            &default_options(),
        );
        assert!(!result.errors.is_empty());
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
    fn source_maps_emitted_when_enabled() {
        let options = Options {
            source_maps: true,
            ..default_options()
        };
        let result = transpile("a.ts", "export const x: number = 1;\n", &options);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let map = result.js_map.unwrap();
        assert!(map.contains("\"a.ts\""), "map: {map}");
    }

    #[test]
    fn transpile_plain_js() {
        let result = transpile("a.js", "export const x = 1;\n", &js_options());
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
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

    fn test_dir(name: &str) -> PathBuf {
        let base = std::env::var_os("TEST_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = base.join(name);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolve_specifier_ignores_bare_and_extensioned() {
        let dir = test_dir("resolve_ignores");
        assert_eq!(resolve_specifier(&dir, "lodash", &[]), None);
        assert_eq!(resolve_specifier(&dir, "./b.js", &[]), None);
        assert_eq!(resolve_specifier(&dir, "./b.ts", &[]), None);
    }

    #[test]
    fn resolve_specifier_resolves_sibling_file() {
        let dir = test_dir("resolve_sibling");
        fs::write(dir.join("b.ts"), "").unwrap();
        fs::write(dir.join("c.mts"), "").unwrap();
        fs::write(dir.join("d.jsx"), "").unwrap();
        fs::write(dir.join("e.cjs"), "").unwrap();
        assert_eq!(resolve_specifier(&dir, "./b", &[]), Some("./b.js".to_string()));
        assert_eq!(resolve_specifier(&dir, "./c", &[]), Some("./c.mjs".to_string()));
        assert_eq!(resolve_specifier(&dir, "./d", &[]), Some("./d.js".to_string()));
        assert_eq!(resolve_specifier(&dir, "./e", &[]), Some("./e.cjs".to_string()));
        assert_eq!(resolve_specifier(&dir, "./missing", &[]), None);
    }

    #[test]
    fn resolve_specifier_prefers_ts_over_js() {
        let dir = test_dir("resolve_prefers_ts");
        fs::write(dir.join("b.mts"), "").unwrap();
        fs::write(dir.join("b.js"), "").unwrap();
        assert_eq!(resolve_specifier(&dir, "./b", &[]), Some("./b.mjs".to_string()));
    }

    #[test]
    fn resolve_specifier_resolves_directory_index() {
        let dir = test_dir("resolve_index");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/index.ts"), "").unwrap();
        assert_eq!(
            resolve_specifier(&dir, "./sub", &[]),
            Some("./sub/index.js".to_string())
        );
    }

    #[test]
    fn resolve_specifier_prefers_file_over_index() {
        let dir = test_dir("resolve_prefers_file");
        fs::write(dir.join("b.ts"), "").unwrap();
        fs::create_dir_all(dir.join("b")).unwrap();
        fs::write(dir.join("b/index.ts"), "").unwrap();
        assert_eq!(resolve_specifier(&dir, "./b", &[]), Some("./b.js".to_string()));
    }

    #[test]
    fn resolve_specifier_across_root_dirs() {
        let dir = test_dir("resolve_root_dirs");
        fs::create_dir_all(dir.join("app")).unwrap();
        fs::create_dir_all(dir.join("lib/sub")).unwrap();
        fs::write(dir.join("lib/shared.ts"), "").unwrap();
        fs::write(dir.join("lib/gen.d.ts"), "").unwrap();
        fs::write(dir.join("lib/genm.d.mts"), "").unwrap();
        fs::write(dir.join("lib/sub/index.ts"), "").unwrap();
        let roots = [dir.join("app"), dir.join("lib")];
        let base = dir.join("app");
        assert_eq!(
            resolve_specifier(&base, "./shared", &roots),
            Some("./shared.js".to_string())
        );
        assert_eq!(
            resolve_specifier(&base, "./gen", &roots),
            Some("./gen.js".to_string())
        );
        assert_eq!(
            resolve_specifier(&base, "./genm", &roots),
            Some("./genm.mjs".to_string())
        );
        assert_eq!(
            resolve_specifier(&base, "./sub", &roots),
            Some("./sub/index.js".to_string())
        );
        assert_eq!(resolve_specifier(&base, "./missing", &roots), None);
    }

    #[test]
    fn resolve_specifier_across_root_dirs_subdir() {
        let dir = test_dir("resolve_root_dirs_subdir");
        fs::create_dir_all(dir.join("app/deep")).unwrap();
        fs::create_dir_all(dir.join("lib/deep")).unwrap();
        fs::write(dir.join("lib/deep/util.ts"), "").unwrap();
        let roots = [dir.join("app"), dir.join("lib")];
        assert_eq!(
            resolve_specifier(&dir.join("app/deep"), "./util", &roots),
            Some("./util.js".to_string())
        );
    }

    #[test]
    fn resolve_specifier_prefers_own_dir_over_other_roots() {
        let dir = test_dir("resolve_root_dirs_own_dir");
        fs::create_dir_all(dir.join("app")).unwrap();
        fs::create_dir_all(dir.join("lib")).unwrap();
        fs::write(dir.join("app/x.ts"), "").unwrap();
        fs::write(dir.join("lib/x.mts"), "").unwrap();
        let roots = [dir.join("app"), dir.join("lib")];
        assert_eq!(
            resolve_specifier(&dir.join("app"), "./x", &roots),
            Some("./x.js".to_string())
        );
    }

    // A directory's own index file wins over a file at the same specifier in a later root: each
    // candidate dir is resolved completely before moving on, matching tsc.
    #[test]
    fn resolve_specifier_own_index_beats_other_root_file() {
        let dir = test_dir("resolve_root_dirs_index_first");
        fs::create_dir_all(dir.join("app/x")).unwrap();
        fs::create_dir_all(dir.join("generated")).unwrap();
        fs::write(dir.join("app/x/index.ts"), "").unwrap();
        fs::write(dir.join("generated/x.mts"), "").unwrap();
        let roots = [dir.join("app"), dir.join("generated")];
        assert_eq!(
            resolve_specifier(&dir.join("app"), "./x", &roots),
            Some("./x/index.js".to_string())
        );
    }

    // With overlapping roots only the longest root containing the source maps to the others:
    // a source under src/gen must not be treated as living at src's "gen" subdirectory.
    #[test]
    fn resolve_specifier_uses_longest_matching_root() {
        let dir = test_dir("resolve_root_dirs_longest");
        fs::create_dir_all(dir.join("src/gen")).unwrap();
        fs::create_dir_all(dir.join("other/gen")).unwrap();
        fs::write(dir.join("src/x.ts"), "").unwrap();
        fs::write(dir.join("other/gen/x.tsx"), "").unwrap();
        let roots = [dir.join("src"), dir.join("src/gen"), dir.join("other")];
        assert_eq!(
            resolve_specifier(&dir.join("src/gen"), "./x", &roots),
            Some("./x.js".to_string())
        );
    }

    // Roots are matched against the normalized import target, so a "../" specifier crossing out
    // of an overlapping root still maps into the other roots.
    #[test]
    fn resolve_specifier_parent_import_across_roots() {
        let dir = test_dir("resolve_root_dirs_parent");
        fs::create_dir_all(dir.join("src/gen")).unwrap();
        fs::create_dir_all(dir.join("other")).unwrap();
        fs::write(dir.join("other/x.tsx"), "").unwrap();
        let roots = [dir.join("src"), dir.join("src/gen"), dir.join("other")];
        assert_eq!(
            resolve_specifier(&dir.join("src/gen"), "../x", &roots),
            Some("../x.js".to_string())
        );
    }

    // A directory whose index exists only as a declaration file resolves like tsc, assuming the
    // JS implementation exists at runtime.
    #[test]
    fn resolve_specifier_resolves_declaration_index() {
        let dir = test_dir("resolve_dts_index");
        fs::create_dir_all(dir.join("gen")).unwrap();
        fs::create_dir_all(dir.join("genm")).unwrap();
        fs::write(dir.join("gen/index.d.ts"), "").unwrap();
        fs::write(dir.join("genm/index.d.mts"), "").unwrap();
        assert_eq!(
            resolve_specifier(&dir, "./gen", &[]),
            Some("./gen/index.js".to_string())
        );
        assert_eq!(
            resolve_specifier(&dir, "./genm", &[]),
            Some("./genm/index.mjs".to_string())
        );
    }

    #[test]
    fn resolve_specifier_outside_roots_unaffected() {
        let dir = test_dir("resolve_root_dirs_outside");
        fs::create_dir_all(dir.join("app")).unwrap();
        fs::create_dir_all(dir.join("other")).unwrap();
        fs::write(dir.join("app/y.ts"), "").unwrap();
        let roots = [dir.join("app"), dir.join("lib")];
        assert_eq!(resolve_specifier(&dir.join("other"), "./y", &roots), None);
    }

    #[test]
    fn resolve_specifier_resolves_sibling_declaration() {
        let dir = test_dir("resolve_sibling_dts");
        fs::write(dir.join("gen.d.ts"), "").unwrap();
        fs::write(dir.join("genc.d.cts"), "").unwrap();
        assert_eq!(resolve_specifier(&dir, "./gen", &[]), Some("./gen.js".to_string()));
        assert_eq!(
            resolve_specifier(&dir, "./genc", &[]),
            Some("./genc.cjs".to_string())
        );
    }

    #[test]
    fn transpile_resolves_across_root_dirs() {
        let dir = test_dir("transpile_root_dirs");
        fs::create_dir_all(dir.join("app")).unwrap();
        fs::create_dir_all(dir.join("lib")).unwrap();
        fs::write(dir.join("lib/shared.ts"), "").unwrap();
        let options = Options {
            root_dirs: vec![dir.join("app"), dir.join("lib")],
            ..js_options()
        };
        let src = dir.join("app/main.ts");
        let js = transpile_js(src.to_str().unwrap(), "export * from \"./shared\";\n", &options);
        assert!(js.contains("export * from \"./shared.js\""), "js: {js}");
    }

    #[test]
    fn run_requires_root_dirs_value() {
        let err = run(args(&["--emit-js", "--root-dirs"])).unwrap_err();
        assert_eq!(err, vec!["error: --root-dirs requires a path".to_string()]);
    }

    #[test]
    fn transpile_resolves_export_all_specifier() {
        let dir = test_dir("transpile_export_all");
        fs::write(dir.join("b.ts"), "").unwrap();
        let src = dir.join("a.ts");
        let js = transpile_js(src.to_str().unwrap(), "export * from \"./b\";\n", &js_options());
        assert!(js.contains("export * from \"./b.js\""), "js: {js}");
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
        let result = transpile("a.ts", "const a = 1;\nconst a = 2;\n", &default_options());
        assert!(!result.errors.is_empty());
        assert!(result.js_code.is_none());
    }

    #[test]
    fn transpile_module_variant_sources() {
        let result = transpile("a.mts", "export const x: number = 1;\n", &default_options());
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert!(result.js_code.unwrap().contains("export const x = 1"));
        assert!(result.dts_code.unwrap().contains("declare const x: number"));

        let result = transpile("a.cts", "export const y: number = 2;\n", &default_options());
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert!(result.js_code.unwrap().contains("y = 2"));
        assert!(result.dts_code.is_some());
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

    #[test]
    fn run_requires_emit_flag() {
        let err = run(args(&[])).unwrap_err();
        assert_eq!(
            err,
            vec!["error: at least one of --emit-js or --emit-dts is required".to_string()]
        );
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

    #[test]
    fn run_passes_through_declaration_files() {
        let dir = test_dir("run_dts_passthrough");
        let src = dir.join("a.d.ts");
        let content = "export declare const x: number;\n";
        fs::write(&src, content).unwrap();
        let dts_out = dir.join("out/a.d.ts");
        let manifest = dir.join("manifest.txt");
        // Declaration entry: empty JS output line, copied to the dts output.
        fs::write(
            &manifest,
            format!("{}\n\n{}\n", src.display(), dts_out.display()),
        )
        .unwrap();
        run(args(&[
            "--emit-js",
            "--emit-dts",
            "--manifest",
            manifest.to_str().unwrap(),
        ]))
        .unwrap();
        assert_eq!(read(&dts_out), content);
    }

    // .d.mts/.d.cts pass through like .d.ts instead of hitting the transpile path.
    #[test]
    fn run_passes_through_module_variant_declaration_files() {
        let dir = test_dir("run_dmts_passthrough");
        for (name, out_name) in [("a.d.mts", "out/a.d.mts"), ("b.d.cts", "out/b.d.cts")] {
            let src = dir.join(name);
            let content = "export declare const x: number;\n";
            fs::write(&src, content).unwrap();
            let dts_out = dir.join(out_name);
            let manifest = dir.join("manifest.txt");
            fs::write(
                &manifest,
                format!("{}\n\n{}\n", src.display(), dts_out.display()),
            )
            .unwrap();
            run(args(&[
                "--emit-js",
                "--emit-dts",
                "--manifest",
                manifest.to_str().unwrap(),
            ]))
            .unwrap();
            assert_eq!(read(&dts_out), content);
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
        assert!(js.ends_with("//# sourceMappingURL=a.js.map"), "js: {js}");
        let map = read(&dir.join("out/a.js.map"));
        assert!(map.contains("a.ts"), "map: {map}");
    }

    #[test]
    fn run_aggregates_errors_across_entries() {
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
        // Both failing entries are reported, and no outputs are written.
        assert!(err.len() >= 2, "errors: {err:?}");
        assert!(!good_out.exists());
    }
}
