"""OXC transpiler rule producing JavaScript and/or TypeScript declaration outputs."""

load("@aspect_rules_js//js:providers.bzl", "JsInfo")

# buildifier: disable=bzl-visibility
load("@aspect_rules_ts//ts/private:ts_lib.bzl", "lib")

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

    return _out_path(path, out_dir, root_dir)

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

def _run_transpile(ctx, srcs, js_outs, dts_outs, map_outs):
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
        if ctx.attr.rewrite_extensions:
            args.add("--rewrite-extensions")
        if ctx.attr.target:
            args.add("--target", ctx.attr.target)
        if ctx.attr.module:
            args.add("--module", ctx.attr.module)
        if ctx.attr.helpers_module:
            args.add("--helpers-module", ctx.attr.helpers_module)
    if emit_dts:
        args.add("--emit-dts")
    args.add("--manifest")
    args.add(manifest)

    ctx.actions.run(
        inputs = srcs + [manifest],
        arguments = [args],
        mnemonic = "OxcTranspile",
        executable = ctx.executable._tool,
        outputs = js_outs + [out for out in dts_outs if out] + map_outs,
        execution_requirements = {"supports-path-mapping": "1"},
    )

def _oxc_transpiler_impl(ctx):
    if not ctx.attr.emit_js and not ctx.attr.emit_dts:
        fail("At least one of emit_js or emit_dts must be True.")

    if ctx.attr.out_dir != "" and ctx.attr.root_dir == "":
        fail("When out_dir is set, root_dir must also be set.")

    if ctx.attr.declaration_dir != "" and ctx.attr.root_dir == "":
        fail("When declaration_dir is set, root_dir must also be set.")

    declaration_dir = ctx.attr.declaration_dir or ctx.attr.out_dir

    root_dir = ctx.attr.root_dir.removesuffix("/")
    if root_dir == ".":
        root_dir = ""

    predeclared = {f.short_path: f for f in ctx.outputs.js_outs + ctx.outputs.dts_outs + ctx.outputs.json_outs}

    js_outs = []  # JS files only, for the transpile action
    dts_outs = []  # declaration files only, for JsInfo types
    json_outs = []  # data files copied through unchanged, alongside JS outputs
    transpile_srcs = []
    transpile_js_outs = []  # aligned with transpile_srcs
    transpile_dts_outs = []  # aligned with transpile_srcs
    map_outs = []

    src_paths = lib.files_relative_to_package(ctx, ctx.files.srcs)

    for src, src_path in zip(ctx.files.srcs, src_paths):
        if _is_declaration(src_path):
            continue

        if root_dir != "" and not src_path.startswith(root_dir + "/"):
            fail(
                "Files to transpile must be in root_dir: \"{}\".\n".format(ctx.attr.root_dir) +
                "The file \"{}\" would produce output outside out_dir: \"{}\".\n".format(src_path, ctx.attr.out_dir),
            )

        # JSON has no syntax to transpile: copy it through unchanged, like tsc's
        # resolveJsonModule emit and swc's equivalent data-src handling.
        if src_path.endswith(".json"):
            if ctx.attr.emit_js:
                out_path = lib.to_out_path(src_path, ctx.attr.out_dir, ctx.attr.root_dir)
                out = _declare(ctx, predeclared, out_path)
                ctx.actions.symlink(output = out, target_file = src)
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

    if transpile_srcs:
        _run_transpile(ctx, transpile_srcs, transpile_js_outs, transpile_dts_outs, map_outs)

    # Mirror the ts_project provider shape: source maps count as sources,
    # DefaultInfo falls back to types when no JS is emitted, declarations are
    # exposed via JsInfo types and the "types" output group, and transitive
    # depsets include JsInfo carried by srcs targets.
    output_sources = js_outs + map_outs + json_outs
    output_types = dts_outs
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
        "out_dir": attr.string(),
        "declaration_dir": attr.string(
            doc = "Directory for the .d.ts outputs, like tsc's declarationDir. " +
                  "Defaults to out_dir.",
        ),
        "root_dir": attr.string(),
        "emit_js": attr.bool(
            default = True,
            doc = "Emit JavaScript outputs.",
        ),
        "emit_dts": attr.bool(
            default = False,
            doc = "Emit .d.ts declaration outputs.",
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
        "helpers_module": attr.string(
            doc = "Module to import runtime helpers from, defaulting to " +
                  "@oxc-project/runtime. Transforms that need a helper always " +
                  "import it, like tsc's importHelpers; oxc has no inline " +
                  "helper mode, so tsc's default importHelpers=false behavior " +
                  "cannot be reproduced. The helpers module must be a runtime " +
                  "dependency whenever a transform emits helper imports (none " +
                  "do in the default configuration).",
        ),
        "jsx": attr.string(
            doc = "JSX runtime, using oxc's values. \"automatic\" (default) is " +
                  "the automatic jsx-runtime transform, tsc's jsx=react-jsx. " +
                  "\"classic\" compiles JSX to React.createElement calls, tsc's " +
                  "jsx=react; providing React is the caller's concern. Map " +
                  "tsc's jsx value spellings to oxc's manually.",
            values = ["", "automatic", "classic"],
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
        "_tool": attr.label(
            executable = True,
            default = "//oxc/private:transpiler",
            cfg = "exec",
        ),
    },
)

def oxc_transpiler(
        name,
        srcs,
        out_dir = "",
        declaration_dir = "",
        root_dir = "",
        emit_js = True,
        emit_dts = False,
        source_maps = False,
        rewrite_extensions = False,
        target = "",
        helpers_module = "",
        jsx = "",
        module = "",
        **kwargs):
    """Macro wrapping _oxc_transpiler_rule that pre-declares output files at load time."""
    _oxc_transpiler_rule(
        name = name,
        srcs = srcs,
        js_outs = _calculate_outs(srcs, _to_js_out, out_dir or "", root_dir or "") if emit_js else [],
        dts_outs = _calculate_outs(srcs, _to_dts_out, declaration_dir or out_dir or "", root_dir or "") if emit_dts else [],
        json_outs = _calculate_outs(srcs, _to_json_out, out_dir or "", root_dir or "") if emit_js else [],
        out_dir = out_dir or "",
        declaration_dir = declaration_dir or "",
        root_dir = root_dir or "",
        emit_js = emit_js,
        emit_dts = emit_dts,
        source_maps = source_maps or False,
        jsx = jsx or "",
        rewrite_extensions = rewrite_extensions or False,
        target = target or "",
        helpers_module = helpers_module or "",
        module = module or "",
        **kwargs
    )
