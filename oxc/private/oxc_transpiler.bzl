"""OXC transpiler rule producing JavaScript and/or TypeScript declaration outputs."""

load("@aspect_rules_js//js:providers.bzl", "JsInfo")

# buildifier: disable=bzl-visibility
load("@aspect_rules_ts//ts/private:ts_lib.bzl", "lib")
load("@bazel_lib//lib:copy_file.bzl", "COPY_FILE_TOOLCHAINS", "copy_file_action")

# Output extension for each input extension.
# .ts/.tsx/.jsx default to .js: JSX is always transformed, so the output is
# plain JS. Explicit entries cover the module-type variants and .js, which
# keep their own name.
_JS_EXT_MAP = {
    ".cts": ".cjs",
    ".cjs": ".cjs",
    ".mts": ".mjs",
    ".mjs": ".mjs",
    ".js": ".js",
}

_DTS_EXT_MAP = {
    ".cts": ".d.cts",
    ".mts": ".d.mts",
}

# Plain JS inputs: transpiled (JSX transform, specifier resolution, source
# maps) but produce no declarations, having no type annotations.
_PLAIN_JS_EXTS = (".js", ".jsx", ".mjs", ".cjs")

def _is_declaration(path):
    return path.endswith(".d.ts") or path.endswith(".d.mts") or path.endswith(".d.cts")

def _out_path(src, out_dir, root_dir):
    """Compute output path from a source label or relative path string.

    Mirrors the logic of lib.to_out_path so that loading-time and analysis-time
    calculations produce the same paths.
    """
    src = src[src.find(":") + 1:]  # strip "pkg:" prefix if present
    if out_dir and src.startswith(out_dir + "/"):
        return src
    if root_dir:
        src = src.removeprefix(root_dir + "/")
    if out_dir:
        src = out_dir + "/" + src
    return src

def _to_js_out(src, out_dir, root_dir):
    """Return the JS output path for a source path, or None to skip it."""
    path = src[src.find(":") + 1:]

    if _is_declaration(path) or path.endswith(".json"):
        return None

    ext_idx = path.rindex(".")
    src_ext = path[ext_idx:]

    out_ext = _JS_EXT_MAP.get(src_ext, ".js")
    out = _out_path(path[:ext_idx] + out_ext, out_dir, root_dir)
    if out == path:
        fail(
            "The src \"{}\" would produce an output with the same path as the input. ".format(path) +
            "Set out_dir to write outputs to a different directory.",
        )
    return out

def _to_dts_out(src, declaration_dir, root_dir):
    """Return the declaration output path for a source path, or None to skip it."""
    path = src[src.find(":") + 1:]

    if _is_declaration(path) or path.endswith(".json"):
        return None

    ext_idx = path.rindex(".")
    src_ext = path[ext_idx:]

    # Plain JS has no type annotations to emit declarations from.
    if src_ext in _PLAIN_JS_EXTS:
        return None

    out_ext = _DTS_EXT_MAP.get(src_ext, ".d.ts")
    return _out_path(path[:ext_idx] + out_ext, declaration_dir, root_dir)

def _to_json_out(src, out_dir, root_dir):
    """Return the output path for a `.json` source, or None to skip it."""
    path = src[src.find(":") + 1:]

    if not path.endswith(".json"):
        return None

    out = _out_path(path, out_dir, root_dir)
    if out == path:
        fail(
            "The src \"{}\" would be copied to the same path as the input. ".format(path) +
            "Set out_dir to write outputs to a different directory.",
        )
    return out

def _calculate_outs(srcs, to_out, *args):
    outs = []
    for src in srcs:
        src = str(src)

        # Extensionless labels are targets whose files are only known at analysis time.
        basename = src[max(src.rfind("/"), src.rfind(":")) + 1:]
        if "." not in basename:
            continue

        out = to_out(src, *args)
        if out:
            outs.append(out)
    return outs

def _declare(ctx, predeclared, out_path):
    """Return the pre-declared File for out_path, or declare it."""
    short_path = out_path
    if ctx.label.package:
        short_path = ctx.label.package + "/" + out_path
    out = predeclared.get(short_path)
    if out == None:
        out = ctx.actions.declare_file(out_path)
    return out

# Targets below es2022, where tsc defaults useDefineForClassFields to false.
_TARGETS_WITHOUT_DEFINE = ("es6", "es2015", "es2016", "es2017", "es2018", "es2019", "es2020", "es2021")

def _use_define_for_class_fields(ctx):
    """Resolve the tri-state attribute, deriving the default from the configured target."""
    if ctx.attr.use_define_for_class_fields:
        return ctx.attr.use_define_for_class_fields == "true"
    return ctx.attr.target not in _TARGETS_WITHOUT_DEFINE

def _run_transpile(ctx, srcs, js_outs, dts_outs, map_outs, dts_map_outs):
    """Run one transpiler action emitting JS and/or declaration outputs.

    Each manifest entry is the source path followed by its JS output path
    (when emitting JS) and its declaration output path (when emitting dts).
    A dts entry may be None (plain JS sources), written as an empty line.
    """
    emit_dts = ctx.attr.emit_dts and any(dts_outs)

    manifest_lines = []
    for i in range(len(srcs)):
        manifest_lines.append(srcs[i].path)
        if js_outs:
            manifest_lines.append(js_outs[i].path)
        if emit_dts:
            manifest_lines.append(dts_outs[i].path if dts_outs[i] else "")
    manifest = ctx.actions.declare_file(ctx.label.name + "_manifest.txt")
    ctx.actions.write(manifest, "\n".join(manifest_lines) + "\n")

    args = ctx.actions.args()
    if js_outs:
        args.add("--emit-js")
        if ctx.attr.source_maps:
            args.add("--source-maps")
        if ctx.attr.jsx:
            args.add("--jsx", ctx.attr.jsx)
        if ctx.attr.jsx_import_source:
            args.add("--jsx-import-source", ctx.attr.jsx_import_source)
        if ctx.attr.jsx_factory:
            args.add("--jsx-pragma", ctx.attr.jsx_factory)
        if ctx.attr.jsx_fragment_factory:
            args.add("--jsx-pragma-frag", ctx.attr.jsx_fragment_factory)
        if ctx.attr.rewrite_extensions:
            args.add("--rewrite-extensions")
        if ctx.attr.target:
            args.add("--target", ctx.attr.target)
        if ctx.attr.module:
            args.add("--module", ctx.attr.module)
        if ctx.attr.helpers_module:
            args.add("--helpers-module", ctx.attr.helpers_module)
        if _use_define_for_class_fields(ctx):
            args.add("--use-define-for-class-fields")
        if ctx.attr.verbatim_module_syntax:
            args.add("--only-remove-type-imports")
        if ctx.attr.experimental_decorators:
            args.add("--experimental-decorators")
        if ctx.attr.emit_decorator_metadata:
            args.add("--emit-decorator-metadata")
        if not ctx.attr.strict_null_checks:
            args.add("--no-strict-null-checks")
    if emit_dts:
        args.add("--emit-dts")
        if ctx.attr.strip_internal:
            args.add("--strip-internal")
        if ctx.attr.declaration_maps:
            args.add("--declaration-maps")
    args.add("--manifest")
    args.add(manifest)

    ctx.actions.run(
        inputs = srcs + [manifest],
        arguments = [args],
        mnemonic = "OxcTranspile",
        executable = ctx.executable._tool,
        outputs = js_outs + [out for out in dts_outs if out] + map_outs + dts_map_outs,
        execution_requirements = {"supports-path-mapping": "1"},
    )

def _oxc_transpiler_impl(ctx):
    if not ctx.attr.emit_js and not ctx.attr.emit_dts:
        fail("At least one of emit_js or emit_dts must be True.")

    if ctx.attr.out_dir != "" and ctx.attr.root_dir == "":
        fail("When out_dir is set, root_dir must also be set.")

    if ctx.attr.declaration_dir != "" and ctx.attr.root_dir == "":
        fail("When declaration_dir is set, root_dir must also be set.")

    if ctx.attr.jsx_import_source and ctx.attr.jsx == "classic":
        fail("jsx_import_source requires the automatic JSX runtime; unset it or set jsx = \"automatic\".")

    if (ctx.attr.jsx_factory or ctx.attr.jsx_fragment_factory) and ctx.attr.jsx != "classic":
        fail("jsx_factory and jsx_fragment_factory require jsx = \"classic\".")

    if ctx.attr.emit_decorator_metadata and not ctx.attr.experimental_decorators:
        fail("emit_decorator_metadata requires experimental_decorators.")

    if ctx.attr.declaration_maps and not ctx.attr.emit_dts:
        fail("declaration_maps requires emit_dts.")

    declaration_dir = ctx.attr.declaration_dir or ctx.attr.out_dir
    root_dir = "" if ctx.attr.root_dir == "." else ctx.attr.root_dir
    predeclared = {f.short_path: f for f in ctx.outputs.js_outs + ctx.outputs.dts_outs + ctx.outputs.json_outs}

    js_outs = []  # JS files only, for the transpile action
    dts_outs = []  # declaration files only, for JsInfo types
    json_outs = []  # data files copied through unchanged, alongside JS outputs
    transpile_srcs = []
    transpile_js_outs = []  # aligned with transpile_srcs
    transpile_dts_outs = []  # aligned with transpile_srcs
    map_outs = []
    dts_map_outs = []

    src_paths = lib.files_relative_to_package(ctx, ctx.files.srcs)

    for src, src_path in zip(ctx.files.srcs, src_paths):
        if _is_declaration(src_path):
            continue

        if root_dir != "" and not src_path.startswith(root_dir + "/"):
            fail(
                "Files to transpile must be in root_dir: \"{}\".\n".format(ctx.attr.root_dir) +
                "The file \"{}\" would produce output outside out_dir: \"{}\".\n".format(src_path, ctx.attr.out_dir),
            )

        # JSON has no syntax to transpile: with emit_json it is copied through
        # unchanged, like tsc's resolveJsonModule emit. Without it a json src produces nothing,
        # matching tsc/ts_project, where the mistake surfaces in the typecheck instead.
        if src_path.endswith(".json"):
            if ctx.attr.emit_js and ctx.attr.emit_json:
                out_path = lib.to_out_path(src_path, ctx.attr.out_dir, ctx.attr.root_dir)
                out = _declare(ctx, predeclared, out_path)
                copy_file_action(ctx, src, out)
                json_outs.append(out)
            continue

        ext_idx = src_path.rindex(".")
        src_ext = src_path[ext_idx:]
        is_plain_js = src_ext in _PLAIN_JS_EXTS

        # Plain JS produces no declarations, so without JS emit it has no outputs.
        if is_plain_js and not ctx.attr.emit_js:
            continue

        transpile_srcs.append(src)

        if ctx.attr.emit_js:
            out_path = src_path[:ext_idx] + _JS_EXT_MAP.get(src_ext, ".js")
            out_path = lib.to_out_path(out_path, ctx.attr.out_dir, ctx.attr.root_dir)
            out = _declare(ctx, predeclared, out_path)
            js_outs.append(out)
            transpile_js_outs.append(out)
            if ctx.attr.source_maps:
                map_out = ctx.actions.declare_file(out_path + ".map")
                map_outs.append(map_out)

        if ctx.attr.emit_dts:
            if is_plain_js:
                transpile_dts_outs.append(None)
            else:
                out_path = src_path[:ext_idx] + _DTS_EXT_MAP.get(src_ext, ".d.ts")
                out_path = lib.to_out_path(out_path, declaration_dir, ctx.attr.root_dir)
                out = _declare(ctx, predeclared, out_path)
                dts_outs.append(out)
                transpile_dts_outs.append(out)
                if ctx.attr.declaration_maps:
                    dts_map_outs.append(ctx.actions.declare_file(out_path + ".map"))

    if transpile_srcs:
        _run_transpile(ctx, transpile_srcs, transpile_js_outs, transpile_dts_outs, map_outs, dts_map_outs)

    # Mirror the ts_project provider shape: source maps count as sources and
    # declaration maps as types, DefaultInfo falls back to types when no JS is
    # emitted, declarations are exposed via JsInfo types and the "types" output
    # group, and transitive depsets include JsInfo carried by srcs targets.
    output_sources = js_outs + map_outs + json_outs
    output_types = dts_outs + dts_map_outs
    default_outputs = output_sources if len(output_sources) else output_types

    src_js_infos = [src[JsInfo] for src in ctx.attr.srcs if JsInfo in src]

    output_sources_depset = depset(output_sources)
    output_types_depset = depset(output_types)

    return [
        DefaultInfo(
            files = depset(default_outputs),
            runfiles = ctx.runfiles(transitive_files = output_sources_depset),
        ),
        JsInfo(
            target = ctx.label,
            sources = output_sources_depset,
            types = output_types_depset,
            transitive_sources = depset(
                output_sources,
                transitive = [info.transitive_sources for info in src_js_infos],
            ),
            transitive_types = depset(
                output_types,
                transitive = [info.transitive_types for info in src_js_infos],
            ),
            npm_sources = depset(
                transitive = [info.npm_sources for info in src_js_infos],
            ),
            npm_package_store_infos = depset(
                transitive = [info.npm_package_store_infos for info in src_js_infos],
            ),
        ),
        OutputGroupInfo(
            types = output_types_depset,
        ),
    ]

_oxc_transpiler_rule = rule(
    implementation = _oxc_transpiler_impl,
    attrs = {
        "srcs": attr.label_list(
            allow_files = True,
            default = [],
            doc = "TypeScript source files to transpile.",
        ),
        # Pre-declared at loading time so that labels like "dist/src/foo.js" in
        # attributes of downstream targets resolve to these generated files
        # rather than source files that may exist on disk.
        "js_outs": attr.output_list(),
        "dts_outs": attr.output_list(),
        "json_outs": attr.output_list(),
        "root_dir": attr.string(),
        "out_dir": attr.string(),
        "declaration_dir": attr.string(
            doc = "Directory for the .d.ts outputs, like tsc's declarationDir. " +
                  "Defaults to out_dir.",
        ),
        "emit_js": attr.bool(
            default = True,
            doc = "Emit JavaScript outputs.",
        ),
        "emit_dts": attr.bool(
            default = False,
            doc = "Emit .d.ts declaration outputs.",
        ),
        "emit_json": attr.bool(
            default = False,
            doc = "Copy .json srcs into the output layout, like tsc's emit " +
                  "under resolveJsonModule; set this when the tsconfig sets " +
                  "resolveJsonModule. Without it json srcs produce no " +
                  "outputs, matching tsc.",
        ),
        "source_maps": attr.bool(
            default = False,
            doc = "Emit a .js.map source map file alongside each JS output.",
        ),
        "target": attr.string(
            doc = "Downlevel the JS outputs to an ECMAScript target (e.g. " +
                  "\"es2017\"), like tsc's target. Defaults to the latest ES " +
                  "version: syntax is left as written. es5 is not allowed: oxc " +
                  "cannot fully downlevel ES2015 syntax. Transforms that need a " +
                  "runtime helper (e.g. async functions below es2017) import it " +
                  "from @oxc-project/runtime, which must then be added as a " +
                  "runtime dependency. Declaration outputs are unaffected.",
            values = [
                "",
                "es6",
                "es2015",
                "es2016",
                "es2017",
                "es2018",
                "es2019",
                "es2020",
                "es2021",
                "es2022",
                "es2023",
                "es2024",
                "es2025",
                "es2026",
                "esnext",
            ],
        ),
        "module": attr.string(
            doc = "Module format of the JS outputs, using oxc's module values. " +
                  "\"preserve\" (default) keeps the input module syntax. " +
                  "\"esm\" keeps ESM syntax and makes TypeScript's " +
                  "CommonJS-specific syntax (`export =`, `import x = " +
                  "require(...)`) an error; tsc spells this mode " +
                  "es2015/esnext, so map your tsconfig module manually. " +
                  "\"commonjs\" mirrors tsc's module=commonjs for that " +
                  "CommonJS-specific syntax: `export =` becomes " +
                  "`module.exports =`, `import x = require(...)` becomes a " +
                  "require call, and a \"use strict\" directive is added. oxc " +
                  "has no ESM-to-CommonJS transform, so ESM import/export " +
                  "syntax in the sources is an error with \"commonjs\".",
            values = ["", "preserve", "esm", "commonjs"],
        ),
        "jsx": attr.string(
            doc = "JSX runtime, using oxc's values. \"automatic\" (default) is " +
                  "the automatic jsx-runtime transform, tsc's jsx=react-jsx. " +
                  "\"classic\" compiles JSX to React.createElement calls, tsc's " +
                  "jsx=react; providing React is the caller's concern. Map " +
                  "tsc's jsx value spellings to oxc's manually.",
            values = ["", "automatic", "classic"],
        ),
        "jsx_import_source": attr.string(
            doc = "Module the automatic JSX runtime is imported from, tsc's " +
                  "jsxImportSource; \"react\" by default. Only applies to " +
                  "jsx = \"automatic\".",
        ),
        "jsx_factory": attr.string(
            doc = "Function the classic JSX runtime compiles elements to, " +
                  "tsc's jsxFactory; React.createElement by default. Only " +
                  "applies to jsx = \"classic\".",
        ),
        "jsx_fragment_factory": attr.string(
            doc = "Expression the classic JSX runtime compiles fragments to, " +
                  "tsc's jsxFragmentFactory; React.Fragment by default. Only " +
                  "applies to jsx = \"classic\".",
        ),
        "experimental_decorators": attr.bool(
            default = False,
            doc = "tsc's experimentalDecorators: compile decorators with the " +
                  "legacy (pre-TC39) transform, calling the decorate helpers " +
                  "imported from helpers_module. Without it decorators are " +
                  "emitted as written, since oxc has no transform for the " +
                  "standard proposal.",
        ),
        "emit_decorator_metadata": attr.bool(
            default = False,
            doc = "tsc's emitDecoratorMetadata: record design:type, " +
                  "design:paramtypes and design:returntype metadata for " +
                  "decorated members through Reflect.metadata, so a " +
                  "reflect-metadata polyfill must be loaded at runtime. " +
                  "Requires experimental_decorators.",
        ),
        "strict_null_checks": attr.bool(
            default = True,
            doc = "tsc's strictNullChecks, which only affects decorator " +
                  "metadata: when False, `T | null` records T's constructor " +
                  "instead of Object.",
        ),
        "declaration_maps": attr.bool(
            default = False,
            doc = "tsc's declarationMap: emit a .d.ts.map beside each " +
                  "declaration output, exposed with the declarations as " +
                  "types. Requires emit_dts.",
        ),
        "rewrite_extensions": attr.bool(
            default = False,
            doc = "Rewrite import/export specifiers that end in '.ts', '.tsx', " +
                  "'.mts', or '.cts' to their emitted JS extension, using oxc's " +
                  "rewrite_import_extensions transform. Unlike tsc's " +
                  "rewriteRelativeImportExtensions, any slash-containing specifier " +
                  "is rewritten (including bare package paths; see README " +
                  "limitations). Specifiers with other extensions (e.g. '.js') " +
                  "are left untouched.",
        ),
        "use_define_for_class_fields": attr.string(
            doc = "tsc's useDefineForClassFields. \"true\" keeps class fields " +
                  "as field definitions, so a field without an initializer is " +
                  "defined as undefined. \"false\" removes fields without an " +
                  "initializer and assigns the others in the constructor. " +
                  "Unset defaults like tsc: \"true\" for target es2022 and " +
                  "above (including the default, esnext), \"false\" below. " +
                  "Declaration outputs are unaffected.",
            values = ["", "true", "false"],
        ),
        "strip_internal": attr.bool(
            default = False,
            doc = "tsc's stripInternal: omit declarations documented with " +
                  "`/** @internal */` from the .d.ts outputs. JS outputs are " +
                  "unaffected.",
        ),
        "verbatim_module_syntax": attr.bool(
            default = False,
            doc = "tsc's verbatimModuleSyntax: only imports and exports " +
                  "marked `type` are removed, so an import whose bindings " +
                  "are unused after type stripping is kept for its side " +
                  "effects. By default any such import is elided, like tsc " +
                  "without the option.",
        ),
        "helpers_module": attr.string(
            doc = "Module to import runtime helpers from, defaulting to " +
                  "@oxc-project/runtime. Transforms that need a helper always " +
                  "import it, like tsc's importHelpers; oxc has no inline " +
                  "helper mode, so tsc's default importHelpers=false behavior " +
                  "cannot be reproduced. The helpers module must be a runtime " +
                  "dependency whenever a transform emits helper imports (none " +
                  "do in the default configuration).",
        ),
        "_tool": attr.label(
            executable = True,
            default = "//oxc/private:transpiler",
            cfg = "exec",
        ),
    },
    toolchains = COPY_FILE_TOOLCHAINS,
)

def _clean_dir(path):
    """Normalize a directory attribute: strip "./" and trailing slashes; "." is the package root."""
    if not path:
        return ""
    path = path.removesuffix("/")
    if path in ("", "."):
        return "."
    return path.removeprefix("./")

def _tristate(value):
    """Map None/True/False to the rule's string attribute; a select() of those strings passes through."""
    if value == None:
        return ""
    if type(value) == "bool":
        return "true" if value else "false"
    return value

def oxc_transpiler(
        name,
        srcs,
        root_dir = "",
        out_dir = "",
        declaration_dir = "",
        emit_js = True,
        emit_dts = False,
        emit_json = False,
        source_maps = False,
        target = "",
        module = "",
        jsx = "",
        rewrite_extensions = False,
        helpers_module = "",
        use_define_for_class_fields = None,
        verbatim_module_syntax = False,
        strip_internal = False,
        jsx_import_source = "",
        jsx_factory = "",
        jsx_fragment_factory = "",
        experimental_decorators = False,
        emit_decorator_metadata = False,
        strict_null_checks = True,
        declaration_maps = False,
        **kwargs):
    """Macro wrapping _oxc_transpiler_rule that pre-declares output files at load time.

    Args:
        name: target name.
        srcs: sources to transpile; files or targets carrying JsInfo.
        root_dir: directory the srcs are relative to when computing output paths.
        out_dir: directory for the JS outputs, relative to the package.
        declaration_dir: directory for the declaration outputs, defaulting to out_dir.
        emit_js: emit JavaScript outputs.
        emit_dts: emit .d.ts declaration outputs.
        emit_json: copy .json srcs into the output layout.
        source_maps: emit a .map file alongside each JS output.
        target: ECMAScript target to downlevel the JS outputs to.
        module: module format of the JS outputs.
        jsx: JSX runtime, "automatic" or "classic".
        rewrite_extensions: rewrite .ts-style import extensions to their JS extension.
        helpers_module: module to import runtime helpers from.
        use_define_for_class_fields: tsc's useDefineForClassFields. Defaults like tsc to
            True for target es2022 and above (including the default, esnext) and False below.
            A select() must use the strings "true" and "false".
        verbatim_module_syntax: tsc's verbatimModuleSyntax; keep imports that are unused
            after type stripping instead of eliding them.
        strip_internal: tsc's stripInternal; omit `/** @internal */` declarations from the
            .d.ts outputs.
        jsx_import_source: module the automatic JSX runtime is imported from, tsc's
            jsxImportSource.
        jsx_factory: function the classic runtime compiles elements to, tsc's jsxFactory.
        jsx_fragment_factory: expression the classic runtime compiles fragments to, tsc's
            jsxFragmentFactory.
        experimental_decorators: tsc's experimentalDecorators; compile decorators with the
            legacy transform.
        emit_decorator_metadata: tsc's emitDecoratorMetadata; record design-time type metadata
            for decorated members.
        strict_null_checks: tsc's strictNullChecks, affecting only decorator metadata.
        declaration_maps: tsc's declarationMap; emit a .d.ts.map beside each declaration.
        **kwargs: common attributes forwarded to the rule.
    """
    out_dir = _clean_dir(out_dir)
    declaration_dir = _clean_dir(declaration_dir)
    root_dir = _clean_dir(root_dir)
    _oxc_transpiler_rule(
        name = name,
        srcs = srcs,
        js_outs = _calculate_outs(srcs, _to_js_out, out_dir, root_dir) if emit_js else [],
        dts_outs = _calculate_outs(srcs, _to_dts_out, declaration_dir or out_dir, root_dir) if emit_dts else [],
        json_outs = _calculate_outs(srcs, _to_json_out, out_dir, root_dir) if emit_js and emit_json else [],
        root_dir = root_dir,
        out_dir = out_dir,
        declaration_dir = declaration_dir,
        emit_js = emit_js,
        emit_dts = emit_dts,
        emit_json = emit_json or False,
        source_maps = source_maps or False,
        target = target or "",
        module = module or "",
        jsx = jsx or "",
        rewrite_extensions = rewrite_extensions or False,
        helpers_module = helpers_module or "",
        use_define_for_class_fields = _tristate(use_define_for_class_fields),
        verbatim_module_syntax = verbatim_module_syntax or False,
        strip_internal = strip_internal or False,
        jsx_import_source = jsx_import_source or "",
        jsx_factory = jsx_factory or "",
        jsx_fragment_factory = jsx_fragment_factory or "",
        experimental_decorators = experimental_decorators or False,
        emit_decorator_metadata = emit_decorator_metadata or False,
        strict_null_checks = strict_null_checks,
        declaration_maps = declaration_maps or False,
        **kwargs
    )
