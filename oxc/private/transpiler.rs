use std::fs;
use std::path::Path;
use oxc_allocator::Allocator;
use oxc_codegen::{CodeGenerator, CommentOptions};
use oxc_diagnostics::{GraphicalReportHandler, GraphicalTheme, NamedSource};
use oxc_isolated_declarations::{
    IsolatedDeclarations, IsolatedDeclarationsOptions as OxcIsolatedDeclarationsOptions,
};
use oxc_parser::Parser;
use oxc_span::SourceType;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let input_path = &args[1];
    let output_path = &args[2];

    if input_path.ends_with(".d.ts") {
        fs::copy(input_path, output_path).unwrap();
        return;
    }

    let content = fs::read_to_string(input_path).unwrap();

    let isolated_declarations = get_isolated_declarations(
        input_path,
        &content,
        // TODO(zbarsky): We may want to make these configurable in the future.
        IsolatedDeclarationsOptions {
            strip_internal: false,
            sourcemap: false,
        },
    );

    if !isolated_declarations.errors.is_empty() {
        eprintln!(
            "Found {} {} in {}:",
            isolated_declarations.errors.len(),
            if isolated_declarations.errors.len() == 1 {
                "error"
            } else {
                "errors"
            },
            input_path
        );
        for error in isolated_declarations.errors.iter() {
            eprintln!("{}", error);
        }

        std::process::exit(1);
    }

    let declaration_path = Path::new(output_path);
    fs::create_dir_all(declaration_path.parent().unwrap()).unwrap();

    fs::write(declaration_path, isolated_declarations.code).unwrap();

    if let Some(source_map_text) = isolated_declarations.map {
        let source_map_path = declaration_path.with_extension("d.ts.map");
        fs::write(source_map_path, source_map_text).unwrap();
    }
}

// Adapted from https://github.com/oxc-project/oxc/blob/main/napi/transform/src/isolated_declaration.rs

pub struct IsolatedDeclarationsOptions {
    pub strip_internal: bool,
    pub sourcemap: bool,
}

pub struct IsolatedDeclarationsResult {
    pub code: String,
    pub map: Option<String>,
    pub errors: Vec<String>,
}

pub fn get_isolated_declarations(
    filename: &str,
    source_text: &str,
    options: IsolatedDeclarationsOptions,
) -> IsolatedDeclarationsResult {
    let source_type = SourceType::from_path(filename)
        .unwrap_or_default()
        .with_typescript(true);

    let allocator = Allocator::default();
    let parser = Parser::new(&allocator, source_text, source_type).parse();

    let isolated_declaration_result = IsolatedDeclarations::new(
        &allocator,
        source_text,
        &parser.trivias,
        OxcIsolatedDeclarationsOptions {
            strip_internal: options.strip_internal,
        },
    )
    .build(&parser.program);

    let code_generator = CodeGenerator::new().enable_comment(
        source_text,
        parser.trivias,
        CommentOptions {
            preserve_annotate_comments: false,
        },
    );

    let code_generator = if options.sourcemap {
        code_generator.enable_source_map(filename, source_text)
    } else {
        code_generator
    };

    let code_gen_result = code_generator.build(&isolated_declaration_result.program);

    let mut errors = vec![];
    if !parser.errors.is_empty() || !isolated_declaration_result.errors.is_empty() {
        let handler = GraphicalReportHandler::new().with_theme(GraphicalTheme::unicode());
        let source = NamedSource::new(filename, source_text.to_string());

        errors.extend(
            parser
                .errors
                .into_iter()
                .chain(isolated_declaration_result.errors)
                .map(|diagnostic| {
                    let diagnostic = diagnostic.with_source_code(source.clone());

                    let mut error_string = String::new();

                    handler
                        .render_report(&mut error_string, diagnostic.as_ref())
                        .unwrap();

                    error_string
                }),
        );
    }

    IsolatedDeclarationsResult {
        code: code_gen_result.code,
        map: code_gen_result
            .map
            .map(|source_map| source_map.to_json_string()),
        errors,
    }
}
