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

enum Mode {
    Js {
        rewrite_extensions: bool,
        source_maps: bool,
    },
    Dts,
}

fn main() {
    let mut mode: Option<Mode> = None;
    let mut manifest_path: Option<String> = None;
    let mut rewrite_extensions = false;
    let mut source_maps = false;
    let mut positional: Vec<String> = Vec::new();
    let mut args_iter = std::env::args().skip(1).peekable();

    while let Some(arg) = args_iter.next() {
        match arg.as_str() {
            "--mode" => {
                let value = args_iter.next().unwrap_or_else(|| {
                    eprintln!("error: --mode requires js or dts");
                    std::process::exit(1);
                });
                mode = Some(match value.as_str() {
                    "js" => Mode::Js {
                        rewrite_extensions: false,
                        source_maps: false,
                    },
                    "dts" => Mode::Dts,
                    other => {
                        eprintln!("error: unknown mode '{other}', expected js or dts");
                        std::process::exit(1);
                    }
                });
            }
            "--rewrite-extensions" => rewrite_extensions = true,
            "--source-maps" => source_maps = true,
            "--manifest" => {
                manifest_path = Some(args_iter.next().unwrap_or_else(|| {
                    eprintln!("error: --manifest requires a file path");
                    std::process::exit(1);
                }));
            }
            _ => positional.push(arg),
        }
    }

    let mode = match mode {
        Some(Mode::Js { .. }) => Mode::Js {
            rewrite_extensions,
            source_maps,
        },
        Some(Mode::Dts) => Mode::Dts,
        None => {
            eprintln!("error: --mode js|dts is required");
            std::process::exit(1);
        }
    };

    let pairs: Vec<(String, String)> = if let Some(path) = manifest_path {
        let content = fs::read_to_string(&path).unwrap_or_else(|e| {
            eprintln!("error: cannot read manifest {path}: {e}");
            std::process::exit(1);
        });
        let lines: Vec<&str> = content.lines().collect();
        if !lines.len().is_multiple_of(2) {
            eprintln!(
                "error: manifest must contain an even number of lines (src/dst pairs), got {}",
                lines.len()
            );
            std::process::exit(1);
        }
        lines
            .chunks(2)
            .map(|c| (c[0].to_string(), c[1].to_string()))
            .collect()
    } else {
        if !positional.len().is_multiple_of(2) {
            eprintln!(
                "error: expected an even number of positional arguments (src dst pairs), got {}",
                positional.len()
            );
            std::process::exit(1);
        }
        positional
            .chunks(2)
            .map(|c| (c[0].clone(), c[1].clone()))
            .collect()
    };

    let mut all_errors: Vec<String> = Vec::new();
    let mut outputs: Vec<(String, String, Option<String>)> = Vec::new();

    for (input_path, output_path) in &pairs {
        let content = fs::read_to_string(input_path).unwrap_or_else(|e| {
            eprintln!("error: cannot read {input_path}: {e}");
            std::process::exit(1);
        });

        match &mode {
            Mode::Js {
                rewrite_extensions,
                source_maps,
            } => {
                let result = transpile_js(input_path, &content, *rewrite_extensions, *source_maps);
                if !result.errors.is_empty() {
                    all_errors.extend(result.errors);
                } else {
                    outputs.push((output_path.clone(), result.code, result.map));
                }
            }
            Mode::Dts => {
                if input_path.ends_with(".d.ts") {
                    outputs.push((output_path.clone(), content, None));
                    continue;
                }
                let result = emit_dts(input_path, &content);
                if !result.errors.is_empty() {
                    all_errors.extend(result.errors);
                } else {
                    outputs.push((output_path.clone(), result.code, None));
                }
            }
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
    code: String,
    map: Option<String>,
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

fn transpile_js(
    filename: &str,
    source_text: &str,
    rewrite_extensions: bool,
    source_maps: bool,
) -> TranspileResult {
    let source_type = SourceType::from_path(filename)
        .unwrap_or_default()
        .with_typescript(true);

    let allocator = Allocator::default();
    let mut parser_ret = Parser::new(&allocator, source_text, source_type).parse();

    let semantic_ret = SemanticBuilder::new().build(&parser_ret.program);
    parser_ret.diagnostics.extend(semantic_ret.diagnostics);
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
            rewrite_import_extensions: rewrite_extensions
                .then_some(oxc::transformer::RewriteExtensionsMode::Rewrite),
            ..Default::default()
        },
        ..Default::default()
    };

    let transformer_ret = Transformer::new(&allocator, Path::new(filename), &transform_options)
        .build_with_scoping(scoping, &mut parser_ret.program);

    if !parser_ret.diagnostics.is_empty() || !transformer_ret.diagnostics.is_empty() {
        let errors = render_errors(
            filename,
            source_text,
            parser_ret
                .diagnostics
                .into_iter()
                .chain(transformer_ret.diagnostics),
        );
        return TranspileResult {
            code: String::new(),
            map: None,
            errors,
        };
    }

    resolve_extensionless_specifiers(&mut parser_ret.program, &allocator, Path::new(filename));

    let mut codegen = Codegen::new();
    if source_maps {
        codegen = codegen.with_options(CodegenOptions {
            source_map_path: Some(PathBuf::from(filename)),
            ..Default::default()
        });
    }
    let codegen_ret = codegen.build(&parser_ret.program);

    TranspileResult {
        code: codegen_ret.code,
        map: codegen_ret.map.map(|m| m.to_json_string()),
        errors: vec![],
    }
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

fn emit_dts(filename: &str, source_text: &str) -> TranspileResult {
    let source_type = SourceType::from_path(filename)
        .unwrap_or_default()
        .with_typescript(true);

    let allocator = Allocator::default();
    let parser = Parser::new(&allocator, source_text, source_type).parse();

    let decl_ret = IsolatedDeclarations::new(
        &allocator,
        OxcIsolatedDeclarationsOptions {
            strip_internal: false,
        },
    )
    .build(&parser.program);

    if !parser.diagnostics.is_empty() || !decl_ret.diagnostics.is_empty() {
        let errors = render_errors(
            filename,
            source_text,
            parser.diagnostics.into_iter().chain(decl_ret.diagnostics),
        );
        return TranspileResult {
            code: String::new(),
            map: None,
            errors,
        };
    }

    TranspileResult {
        code: Codegen::new().build(&decl_ret.program).code,
        map: None,
        errors: vec![],
    }
}
