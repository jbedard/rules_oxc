use oxc::allocator::Allocator;
use oxc::ast::ast::{Program, Statement};
use oxc::codegen::{Codegen, CodegenOptions};
use oxc::diagnostics::{GraphicalReportHandler, GraphicalTheme, NamedSource};
use oxc::isolated_declarations::{
    IsolatedDeclarations, IsolatedDeclarationsOptions as OxcIsolatedDeclarationsOptions,
};
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::SourceType;
use oxc::transformer::{
    CompilerAssumptions, JsxOptions, TransformOptions, Transformer, TypeScriptOptions,
};
use std::fs;
use std::path::{Path, PathBuf};

struct Options {
    emit_js: bool,
    emit_dts: bool,
    source_maps: bool,
    preserve_jsx: bool,
    rewrite_extensions: bool,
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

fn run(args: impl Iterator<Item = String>) -> Result<(), Vec<String>> {
    let mut options = Options {
        emit_js: false,
        emit_dts: false,
        source_maps: false,
        preserve_jsx: false,
        rewrite_extensions: false,
    };
    let mut manifest_path: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut args_iter = args;

    while let Some(arg) = args_iter.next() {
        match arg.as_str() {
            "--emit-js" => options.emit_js = true,
            "--emit-dts" => options.emit_dts = true,
            "--source-maps" => options.source_maps = true,
            "--preserve-jsx" => options.preserve_jsx = true,
            "--rewrite-extensions" => options.rewrite_extensions = true,
            "--manifest" => {
                manifest_path = Some(
                    args_iter
                        .next()
                        .ok_or_else(|| vec!["error: --manifest requires a file path".to_string()])?,
                );
            }
            _ => positional.push(arg),
        }
    }

    if !options.emit_js && !options.emit_dts {
        return Err(vec![
            "error: at least one of --emit-js or --emit-dts is required".to_string(),
        ]);
    }

    // Each manifest entry is the source path followed by the JS output path
    // (when --emit-js) and the declaration output path (when --emit-dts).
    // An empty output path means that output is skipped for the entry
    // (e.g. no declarations for plain JS sources).
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
    let mut outputs: Vec<(String, String, Option<String>)> = Vec::new();

    for entry in &entries {
        let content = fs::read_to_string(&entry.src)
            .map_err(|e| vec![format!("error: cannot read {}: {e}", entry.src)])?;

        // Declaration files pass through unchanged and have no JS output.
        if entry.src.ends_with(".d.ts") {
            if let Some(dts_out) = &entry.dts_out {
                outputs.push((dts_out.clone(), content, None));
            }
            continue;
        }

        let entry_options = Options {
            emit_js: entry.js_out.is_some(),
            emit_dts: entry.dts_out.is_some(),
            source_maps: options.source_maps,
            preserve_jsx: options.preserve_jsx,
            rewrite_extensions: options.rewrite_extensions,
        };
        let result = transpile(&entry.src, &content, &entry_options);
        if !result.errors.is_empty() {
            all_errors.extend(result.errors);
            continue;
        }
        if let (Some(js_out), Some(code)) = (&entry.js_out, result.js_code) {
            outputs.push((js_out.clone(), code, result.js_map));
        }
        if let (Some(dts_out), Some(code)) = (&entry.dts_out, result.dts_code) {
            outputs.push((dts_out.clone(), code, None));
        }
    }

    if !all_errors.is_empty() {
        return Err(all_errors);
    }

    for (output_path, code, map) in outputs {
        let out = Path::new(&output_path);
        fs::create_dir_all(out.parent().unwrap()).unwrap();
        if let Some(map) = map {
            let map_basename = out.file_name().unwrap().to_string_lossy();
            let code = format!("{code}\n//# sourceMappingURL={map_basename}.map");
            fs::write(out, code).unwrap();
            fs::write(format!("{output_path}.map"), map).unwrap();
        } else {
            fs::write(out, code).unwrap();
        }
    }

    Ok(())
}

struct TranspileResult {
    js_code: Option<String>,
    js_map: Option<String>,
    dts_code: Option<String>,
    errors: Vec<String>,
}

fn render_errors(
    filename: &str,
    source_text: &str,
    diagnostics: impl Iterator<Item = oxc::diagnostics::OxcDiagnostic>,
) -> Vec<String> {
    let handler = GraphicalReportHandler::new().with_theme(GraphicalTheme::none());
    let source = NamedSource::new(filename, source_text.to_string());
    diagnostics
        .map(|diagnostic| {
            let diagnostic = diagnostic.with_source_code(source.clone());
            let mut s = String::new();
            handler.render_report(&mut s, diagnostic.as_ref()).unwrap();
            s
        })
        .collect()
}

// Parses once and emits JS and/or declaration outputs from the same AST.
// Declarations are emitted first, before the transformer mutates the program.
fn transpile(filename: &str, source_text: &str, options: &Options) -> TranspileResult {
    let source_type = SourceType::from_path(filename)
        .unwrap_or_default()
        .with_typescript(true);

    let allocator = Allocator::default();
    let mut parser_ret = Parser::new(&allocator, source_text, source_type).parse();

    let mut result = TranspileResult {
        js_code: None,
        js_map: None,
        dts_code: None,
        errors: vec![],
    };

    if !parser_ret.diagnostics.is_empty() {
        result.errors = render_errors(filename, source_text, parser_ret.diagnostics.into_iter());
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
            result.errors = render_errors(filename, source_text, decl_ret.diagnostics.into_iter());
            return result;
        }

        result.dts_code = Some(Codegen::new().build(&decl_ret.program).code);
    }

    if options.emit_js {
        let semantic_ret = SemanticBuilder::new().build(&parser_ret.program);
        let semantic_diagnostics = semantic_ret.diagnostics;
        let scoping = semantic_ret.semantic.into_scoping();

        let transform_options = TransformOptions {
            // Match the SWC useDefineForClassFields=false behaviour:
            // class fields without initializers are removed rather than set to undefined.
            assumptions: CompilerAssumptions {
                set_public_class_fields: true,
                ..Default::default()
            },
            typescript: TypeScriptOptions {
                remove_class_fields_without_initializer: true,
                ..Default::default()
            },
            // Like tsc's jsx=preserve: strip types but leave JSX untouched.
            jsx: if options.preserve_jsx {
                JsxOptions::disable()
            } else {
                JsxOptions::default()
            },
            ..Default::default()
        };

        let transformer_ret = Transformer::new(&allocator, Path::new(filename), &transform_options)
            .build_with_scoping(scoping, &mut parser_ret.program);

        let diagnostics: Vec<_> = semantic_diagnostics
            .into_iter()
            .chain(transformer_ret.diagnostics)
            .collect();
        if !diagnostics.is_empty() {
            result.errors = render_errors(filename, source_text, diagnostics.into_iter());
            return result;
        }

        resolve_relative_specifiers(
            &mut parser_ret.program,
            &allocator,
            Path::new(filename),
            options.preserve_jsx,
            options.rewrite_extensions,
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

// Node's ESM loader requires fully-specified relative specifiers: no directory
// imports and no extensionless imports. TS's "bundler" moduleResolution allows
// omitting the extension (and importing a directory whose index file resolves
// it), so we resolve those against the sibling source files on disk here —
// mirroring what swc's `module.resolveFully` option already does for the swc
// transpiler path.
//
// Separately, sources may already write a fully-specified TypeScript
// extension (e.g. "./foo.ts"), relying on tsc's `rewriteRelativeImportExtensions`
// to rewrite it to the emitted JS extension. When `rewrite_extensions` is set
// we mirror that rewrite too, so this transpiler is a drop-in replacement for
// swc's equivalent `jsc.rewriteRelativeImportExtensions` config.
fn resolve_relative_specifiers<'a>(
    program: &mut Program<'a>,
    allocator: &'a Allocator,
    filename: &Path,
    preserve_jsx: bool,
    rewrite_extensions: bool,
) {
    let base_dir = filename.parent().unwrap_or_else(|| Path::new(""));

    for stmt in program.body.iter_mut() {
        let source = match stmt {
            Statement::ImportDeclaration(decl) => Some(&mut decl.source),
            Statement::ExportFromDeclaration(decl) => Some(&mut decl.source),
            Statement::ExportAllDeclaration(decl) => Some(&mut decl.source),
            _ => None,
        };

        let Some(source) = source else {
            continue;
        };

        if let Some(resolved) = resolve_specifier(
            base_dir,
            source.value.as_str(),
            preserve_jsx,
            rewrite_extensions,
        ) {
            source.value = allocator.alloc_str(&resolved).into();
            source.raw = None;
        }
    }
}

// TypeScript sources first: with both foo.ts and foo.js present, tsc
// resolves "./foo" to foo.ts.
const RESOLVABLE_EXTS: [&str; 8] = ["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

fn js_ext_for(src_ext: &str, preserve_jsx: bool) -> &'static str {
    match src_ext {
        "mts" | "mjs" => "mjs",
        "cts" | "cjs" => "cjs",
        "tsx" | "jsx" if preserve_jsx => "jsx",
        _ => "js",
    }
}

// TypeScript source extensions that `rewriteRelativeImportExtensions` rewrites
// to their JS equivalent. Other extensions (.js, .jsx, .mjs, .cjs, ...) are
// already valid runtime specifiers and are left untouched, matching tsc.
const REWRITABLE_TS_EXTS: [&str; 4] = ["ts", "tsx", "mts", "cts"];

fn resolve_specifier(
    base_dir: &Path,
    specifier: &str,
    preserve_jsx: bool,
    rewrite_extensions: bool,
) -> Option<String> {
    if !specifier.starts_with("./") && !specifier.starts_with("../") {
        return None;
    }

    if let Some(ext) = Path::new(specifier).extension().and_then(|e| e.to_str()) {
        if rewrite_extensions && REWRITABLE_TS_EXTS.contains(&ext) {
            let stem = specifier.strip_suffix(&format!(".{ext}"))?;
            return Some(format!("{stem}.{}", js_ext_for(ext, preserve_jsx)));
        }
        // Already has a non-rewritable extension (or rewriting is disabled): leave as-is.
        return None;
    }

    let target = base_dir.join(specifier);

    for ext in RESOLVABLE_EXTS {
        if target.with_extension(ext).is_file() {
            return Some(format!("{specifier}.{}", js_ext_for(ext, preserve_jsx)));
        }
    }

    for ext in RESOLVABLE_EXTS {
        if target.join(format!("index.{ext}")).is_file() {
            return Some(format!(
                "{specifier}/index.{}",
                js_ext_for(ext, preserve_jsx)
            ));
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
            preserve_jsx: false,
            rewrite_extensions: false,
        }
    }

    #[test]
    fn js_ext_mapping() {
        assert_eq!(js_ext_for("ts", false), "js");
        assert_eq!(js_ext_for("tsx", false), "js");
        assert_eq!(js_ext_for("mts", false), "mjs");
        assert_eq!(js_ext_for("cts", false), "cjs");
        assert_eq!(js_ext_for("js", false), "js");
        assert_eq!(js_ext_for("jsx", false), "js");
        assert_eq!(js_ext_for("mjs", false), "mjs");
        assert_eq!(js_ext_for("cjs", false), "cjs");
        assert_eq!(js_ext_for("tsx", true), "jsx");
        assert_eq!(js_ext_for("jsx", true), "jsx");
        assert_eq!(js_ext_for("ts", true), "js");
    }

    #[test]
    fn transpile_strips_types() {
        let result = transpile(
            "a.ts",
            "export const x: number = 1;\nexport interface I { a: string }\n",
            &default_options(),
        );
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let js = result.js_code.unwrap();
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
        let result = transpile(
            "a.ts",
            "export class C { declared: number; assigned = 1; }\n",
            &default_options(),
        );
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let js = result.js_code.unwrap();
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
        let options = Options {
            emit_dts: false,
            ..default_options()
        };
        let result = transpile("a.js", "export const x = 1;\n", &options);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let js = result.js_code.unwrap();
        assert!(js.contains("export const x = 1"), "js: {js}");
        assert!(result.dts_code.is_none());
    }

    #[test]
    fn transpile_transforms_jsx() {
        let options = Options {
            emit_dts: false,
            ..default_options()
        };
        let result = transpile(
            "a.tsx",
            "export const el: object = <div id={1} />;\n",
            &options,
        );
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let js = result.js_code.unwrap();
        assert!(js.contains("react/jsx-runtime"), "js: {js}");
        assert!(js.contains("_jsx("), "js: {js}");
        assert!(!js.contains(": object"), "js: {js}");
    }

    #[test]
    fn transpile_transforms_jsx_in_plain_jsx_file() {
        let options = Options {
            emit_dts: false,
            ..default_options()
        };
        let result = transpile("a.jsx", "export const el = <div id={1} />;\n", &options);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let js = result.js_code.unwrap();
        assert!(js.contains("_jsx("), "js: {js}");
    }

    #[test]
    fn preserve_jsx_keeps_jsx_syntax() {
        let options = Options {
            emit_dts: false,
            preserve_jsx: true,
            ..default_options()
        };
        let result = transpile(
            "a.tsx",
            "export const el: object = <div id={1} />;\n",
            &options,
        );
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let js = result.js_code.unwrap();
        assert!(js.contains("<div"), "js: {js}");
        assert!(!js.contains("_jsx("), "js: {js}");
        assert!(!js.contains("react/jsx-runtime"), "js: {js}");
        assert!(!js.contains(": object"), "js: {js}");
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
        assert_eq!(resolve_specifier(&dir, "lodash", false, false), None);
        assert_eq!(resolve_specifier(&dir, "./b.js", false, false), None);
        assert_eq!(resolve_specifier(&dir, "./b.ts", false, false), None);
    }

    #[test]
    fn resolve_specifier_resolves_sibling_file() {
        let dir = test_dir("resolve_sibling");
        fs::write(dir.join("b.ts"), "").unwrap();
        fs::write(dir.join("c.mts"), "").unwrap();
        fs::write(dir.join("d.jsx"), "").unwrap();
        fs::write(dir.join("e.cjs"), "").unwrap();
        assert_eq!(resolve_specifier(&dir, "./b", false, false), Some("./b.js".to_string()));
        assert_eq!(resolve_specifier(&dir, "./c", false, false), Some("./c.mjs".to_string()));
        assert_eq!(resolve_specifier(&dir, "./d", false, false), Some("./d.js".to_string()));
        assert_eq!(resolve_specifier(&dir, "./e", false, false), Some("./e.cjs".to_string()));
        assert_eq!(resolve_specifier(&dir, "./missing", false, false), None);
    }

    #[test]
    fn resolve_specifier_prefers_ts_over_js() {
        let dir = test_dir("resolve_prefers_ts");
        fs::write(dir.join("b.mts"), "").unwrap();
        fs::write(dir.join("b.js"), "").unwrap();
        assert_eq!(resolve_specifier(&dir, "./b", false, false), Some("./b.mjs".to_string()));
    }

    #[test]
    fn resolve_specifier_resolves_directory_index() {
        let dir = test_dir("resolve_index");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/index.ts"), "").unwrap();
        assert_eq!(
            resolve_specifier(&dir, "./sub", false, false),
            Some("./sub/index.js".to_string())
        );
    }

    #[test]
    fn resolve_specifier_prefers_file_over_index() {
        let dir = test_dir("resolve_prefers_file");
        fs::write(dir.join("b.ts"), "").unwrap();
        fs::create_dir_all(dir.join("b")).unwrap();
        fs::write(dir.join("b/index.ts"), "").unwrap();
        assert_eq!(resolve_specifier(&dir, "./b", false, false), Some("./b.js".to_string()));
    }

    #[test]
    fn resolve_specifier_leaves_ts_extension_when_rewrite_disabled() {
        let dir = test_dir("resolve_rewrite_disabled");
        assert_eq!(resolve_specifier(&dir, "./b.ts", false, false), None);
    }

    #[test]
    fn resolve_specifier_rewrites_ts_extensions_when_enabled() {
        let dir = test_dir("resolve_rewrite_enabled");
        assert_eq!(
            resolve_specifier(&dir, "./b.ts", false, true),
            Some("./b.js".to_string())
        );
        assert_eq!(
            resolve_specifier(&dir, "./b.mts", false, true),
            Some("./b.mjs".to_string())
        );
        assert_eq!(
            resolve_specifier(&dir, "./b.cts", false, true),
            Some("./b.cjs".to_string())
        );
        assert_eq!(
            resolve_specifier(&dir, "./b.tsx", false, true),
            Some("./b.js".to_string())
        );
        assert_eq!(
            resolve_specifier(&dir, "./b.tsx", true, true),
            Some("./b.jsx".to_string())
        );
    }

    #[test]
    fn resolve_specifier_never_rewrites_js_extensions() {
        let dir = test_dir("resolve_rewrite_js_untouched");
        assert_eq!(resolve_specifier(&dir, "./b.js", false, true), None);
        assert_eq!(resolve_specifier(&dir, "./b.jsx", false, true), None);
        assert_eq!(resolve_specifier(&dir, "./b.mjs", false, true), None);
        assert_eq!(resolve_specifier(&dir, "./b.cjs", false, true), None);
    }

    #[test]
    fn transpile_resolves_export_all_specifier() {
        let dir = test_dir("transpile_export_all");
        fs::write(dir.join("b.ts"), "").unwrap();
        let options = Options {
            emit_dts: false,
            ..default_options()
        };
        let src = dir.join("a.ts");
        let result = transpile(src.to_str().unwrap(), "export * from \"./b\";\n", &options);
        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        let js = result.js_code.unwrap();
        assert!(js.contains("export * from \"./b.js\""), "js: {js}");
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
