"""OXC transpiler rule producing JavaScript and/or TypeScript declaration outputs."""

load("@aspect_rules_js//js:providers.bzl", "JsInfo")

# buildifier: disable=bzl-visibility
load("@aspect_rules_ts//ts/private:ts_lib.bzl", "lib")

# Output extension for each input extension.
# .ts/.tsx default to .js/.d.ts; explicit entries cover the module-type variants.
_JS_EXT_MAP = {
    ".cts": ".cjs",
    ".cjs": ".cjs",
    ".mts": ".mjs",
    ".mjs": ".mjs",
}

_DTS_EXT_MAP = {
    ".cts": ".d.cts",
    ".cjs": ".d.cts",
    ".mts": ".d.mts",
    ".mjs": ".d.mts",
}

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

def _to_js_out(src, allow_js, out_dir, root_dir):
    """Return the JS output path for a source path, or None to skip it."""
    path = src[src.find(":") + 1:]

    if _is_declaration(path) or path.endswith(".json"):
        return None

    ext_idx = path.rindex(".")
    src_ext = path[ext_idx:]

    if src_ext in (".js", ".jsx"):
        return _out_path(path, out_dir, root_dir) if allow_js else None

    out_ext = _JS_EXT_MAP.get(src_ext, ".js")
    return _out_path(path[:ext_idx] + out_ext, out_dir, root_dir)

def _to_dts_out(src, out_dir, root_dir):
    """Return the declaration output path for a source path, or None to skip it."""
    path = src[src.find(":") + 1:]

    if _is_declaration(path) or path.endswith(".json"):
        return None

    ext_idx = path.rindex(".")
    src_ext = path[ext_idx:]

    # Plain JS has no type annotations to emit declarations from.
    if src_ext in (".js", ".jsx"):
        return None

    out_ext = _DTS_EXT_MAP.get(src_ext, ".d.ts")
    return _out_path(path[:ext_idx] + out_ext, out_dir, root_dir)

def _calculate_outs(srcs, to_out, *args):
    outs = []
    for src in srcs:
        out = to_out(str(src), *args)
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
    """
    manifest_lines = []
    for i in range(len(srcs)):
        manifest_lines.append(srcs[i].path)
        if js_outs:
            manifest_lines.append(js_outs[i].path)
        if dts_outs:
            manifest_lines.append(dts_outs[i].path)
    manifest = ctx.actions.declare_file(ctx.label.name + "_manifest.txt")
    ctx.actions.write(manifest, "\n".join(manifest_lines) + "\n")

    args = ctx.actions.args()
    if js_outs:
        args.add("--emit-js")
        if ctx.attr.rewrite_extensions:
            args.add("--rewrite-extensions")
        if ctx.attr.source_maps:
            args.add("--source-maps")
    if dts_outs:
        args.add("--emit-dts")
    args.add("--manifest")
    args.add(manifest)

    ctx.actions.run(
        inputs = srcs + [manifest],
        arguments = [args],
        mnemonic = "OxcTranspile",
        executable = ctx.executable._tool,
        outputs = js_outs + dts_outs + map_outs,
        execution_requirements = {"supports-path-mapping": "1"},
    )

def _oxc_transpiler_impl(ctx):
    if not ctx.attr.emit_js and not ctx.attr.emit_dts:
        fail("At least one of emit_js or emit_dts must be True.")

    if ctx.attr.out_dir != "" and ctx.attr.root_dir == "":
        fail("When out_dir is set, root_dir must also be set.")

    root_dir = ctx.attr.root_dir.removesuffix("/")
    if root_dir == ".":
        root_dir = ""

    predeclared = {f.short_path: f for f in ctx.outputs.js_outs + ctx.outputs.dts_outs}

    js_outs = []  # JS files only, for the transpile action
    dts_outs = []  # declaration files only, for JsInfo types
    transpile_srcs = []
    transpile_js_outs = []  # aligned with transpile_srcs
    transpile_dts_outs = []  # aligned with transpile_srcs
    map_outs = []

    src_paths = lib.files_relative_to_package(ctx, ctx.files.srcs)

    for src, src_path in zip(ctx.files.srcs, src_paths):
        if _is_declaration(src_path) or src_path.endswith(".json"):
            continue

        if root_dir != "" and not src_path.startswith(root_dir + "/"):
            fail(
                "Files to transpile must be in root_dir: \"{}\".\n".format(ctx.attr.root_dir) +
                "The file \"{}\" would produce output outside out_dir: \"{}\".\n".format(src_path, ctx.attr.out_dir),
            )

        ext_idx = src_path.rindex(".")
        src_ext = src_path[ext_idx:]

        # Plain JS/JSX files are copied through when allow_js is set; no declarations.
        if src_ext in (".js", ".jsx"):
            if not (ctx.attr.emit_js and ctx.attr.allow_js):
                continue
            out_path = lib.to_out_path(src_path, ctx.attr.out_dir, ctx.attr.root_dir)
            out = _declare(ctx, predeclared, out_path)
            ctx.actions.run_shell(
                inputs = [src],
                outputs = [out],
                mnemonic = "CopyJs",
                command = "cp \"$1\" \"$2\"",
                arguments = [src.path, out.path],
                execution_requirements = {"supports-path-mapping": "1"},
            )
            js_outs.append(out)
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
            out_path = src_path[:ext_idx] + _DTS_EXT_MAP.get(src_ext, ".d.ts")
            out_path = lib.to_out_path(out_path, ctx.attr.out_dir, ctx.attr.root_dir)
            out = _declare(ctx, predeclared, out_path)
            dts_outs.append(out)
            transpile_dts_outs.append(out)

    if transpile_srcs:
        _run_transpile(ctx, transpile_srcs, transpile_js_outs, transpile_dts_outs, map_outs)

    # Mirror the ts_project provider shape: source maps count as sources,
    # DefaultInfo falls back to types when no JS is emitted, declarations are
    # exposed via JsInfo types and the "types" output group, and transitive
    # depsets include JsInfo carried by srcs targets.
    output_sources = js_outs + map_outs
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
        "out_dir": attr.string(),
        "root_dir": attr.string(),
        "emit_js": attr.bool(
            default = True,
            doc = "Emit JavaScript outputs.",
        ),
        "emit_dts": attr.bool(
            default = False,
            doc = "Emit .d.ts declaration outputs.",
        ),
        "allow_js": attr.bool(default = False),
        "rewrite_extensions": attr.bool(
            default = False,
            doc = "Rewrite .ts/.mts/.cts import extensions to .js/.mjs/.cjs.",
        ),
        "source_maps": attr.bool(
            default = False,
            doc = "Emit a .js.map source map file alongside each JS output.",
        ),
        "_tool": attr.label(
            executable = True,
            default = "//private/transpiler",
            cfg = "exec",
        ),
    },
)

def oxc_transpiler(
        name,
        srcs,
        out_dir = "",
        root_dir = "",
        emit_js = True,
        emit_dts = False,
        allow_js = False,
        rewrite_extensions = False,
        source_maps = False,
        **kwargs):
    """Macro wrapping _oxc_transpiler_rule that pre-declares output files at load time."""
    _oxc_transpiler_rule(
        name = name,
        srcs = srcs,
        js_outs = _calculate_outs(srcs, _to_js_out, allow_js or False, out_dir or "", root_dir or "") if emit_js else [],
        dts_outs = _calculate_outs(srcs, _to_dts_out, out_dir or "", root_dir or "") if emit_dts else [],
        out_dir = out_dir or "",
        root_dir = root_dir or "",
        emit_js = emit_js,
        emit_dts = emit_dts,
        allow_js = allow_js or False,
        rewrite_extensions = rewrite_extensions or False,
        source_maps = source_maps or False,
        **kwargs
    )
