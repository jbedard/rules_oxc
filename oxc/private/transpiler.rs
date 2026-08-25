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
use oxc::transformer::{CompilerAssumptions, TransformOptions, Transformer, TypeScriptOptions};
use std::fs;
use std::path::{Path, PathBuf};

struct Options {
    emit_js: bool,
    emit_dts: bool,
    rewrite_extensions: bool,
    source_maps: bool,
}

struct Entry {
    src: String,
    js_out: Option<String>,
    dts_out: Option<String>,
}

fn main() {
    let mut options = Options {
        emit_js: false,
        emit_dts: false,
        rewrite_extensions: false,
        source_maps: false,
    };
    let mut manifest_path: Option<String> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut args_iter = std::env::args().skip(1);

    while let Some(arg) = args_iter.next() {
        match arg.as_str() {
            "--emit-js" => options.emit_js = true,
            "--emit-dts" => options.emit_dts = true,
            "--rewrite-extensions" => options.rewrite_extensions = true,
            "--source-maps" => options.source_maps = true,
            "--manifest" => {
                manifest_path = Some(args_iter.next().unwrap_or_else(|| {
                    eprintln!("error: --manifest requires a file path");
                    std::process::exit(1);
                }));
            }
            _ => positional.push(arg),
        }
    }

    if !options.emit_js && !options.emit_dts {
        eprintln!("error: at least one of --emit-js or --emit-dts is required");
        std::process::exit(1);
    }

    // Each manifest entry is the source path followed by the JS output path
    // (when --emit-js) and the declaration output path (when --emit-dts).
    let entry_width = 1 + options.emit_js as usize + options.emit_dts as usize;

    let lines: Vec<String> = if let Some(path) = manifest_path {
        let content = fs::read_to_string(&path).unwrap_or_else(|e| {
            eprintln!("error: cannot read manifest {path}: {e}");
            std::process::exit(1);
        });
        content.lines().map(str::to_string).collect()
    } else {
        positional
    };

    if !lines.len().is_multiple_of(entry_width) {
        eprintln!(
            "error: expected entries of {entry_width} lines (src followed by output paths), got {} lines",
            lines.len()
        );
        std::process::exit(1);
    }

    let entries: Vec<Entry> = lines
        .chunks(entry_width)
        .map(|chunk| {
            let mut outs = chunk[1..].iter();
            Entry {
                src: chunk[0].clone(),
                js_out: options.emit_js.then(|| outs.next().unwrap().clone()),
                dts_out: options.emit_dts.then(|| outs.next().unwrap().clone()),
            }
        })
        .collect();

    let mut all_errors: Vec<String> = Vec::new();
    let mut outputs: Vec<(String, String, Option<String>)> = Vec::new();

    for entry in &entries {
        let content = fs::read_to_string(&entry.src).unwrap_or_else(|e| {
            eprintln!("error: cannot read {}: {e}", entry.src);
            std::process::exit(1);
        });

        // Declaration files pass through unchanged and have no JS output.
        if entry.src.ends_with(".d.ts") {
            if let Some(dts_out) = &entry.dts_out {
                outputs.push((dts_out.clone(), content, None));
            }
            continue;
        }

        let result = transpile(&entry.src, &content, &options);
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
        for error in &all_errors {
            eprintln!("{error}");
        }
        std::process::exit(1);
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
                rewrite_import_extensions: options
                    .rewrite_extensions
                    .then_some(oxc::transformer::RewriteExtensionsMode::Rewrite),
                ..Default::default()
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

        resolve_extensionless_specifiers(&mut parser_ret.program, &allocator, Path::new(filename));

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
fn resolve_extensionless_specifiers<'a>(
    program: &mut Program<'a>,
    allocator: &'a Allocator,
    filename: &Path,
) {
    let base_dir = filename.parent().unwrap_or_else(|| Path::new(""));

    for stmt in program.body.iter_mut() {
        let source = match stmt {
            Statement::ImportDeclaration(decl) => Some(&mut decl.source),
            Statement::ExportNamedDeclaration(decl) => decl.source.as_mut(),
            Statement::ExportAllDeclaration(decl) => Some(&mut decl.source),
            _ => None,
        };

        let Some(source) = source else {
            continue;
        };

        if let Some(resolved) = resolve_specifier(base_dir, source.value.as_str()) {
            source.value = allocator.alloc_str(&resolved).into();
            source.raw = None;
        }
    }
}

const RESOLVABLE_EXTS: [&str; 4] = ["ts", "tsx", "mts", "cts"];

fn js_ext_for(ts_ext: &str) -> &'static str {
    match ts_ext {
        "mts" => "mjs",
        "cts" => "cjs",
        _ => "js",
    }
}

fn resolve_specifier(base_dir: &Path, specifier: &str) -> Option<String> {
    if !specifier.starts_with("./") && !specifier.starts_with("../") {
        return None;
    }
    // Already has an extension: either already resolvable as-is, or handled by
    // the TypeScript extension rewriter above.
    if Path::new(specifier).extension().is_some() {
        return None;
    }

    let target = base_dir.join(specifier);

    for ext in RESOLVABLE_EXTS {
        if target.with_extension(ext).is_file() {
            return Some(format!("{specifier}.{}", js_ext_for(ext)));
        }
    }

    for ext in RESOLVABLE_EXTS {
        if target.join(format!("index.{ext}")).is_file() {
            return Some(format!("{specifier}/index.{}", js_ext_for(ext)));
        }
    }

    None
}
