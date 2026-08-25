"""TypeScript-to-JavaScript transpiler using OXC."""

load("@aspect_rules_js//js:providers.bzl", "JsInfo")

# buildifier: disable=bzl-visibility
load("@aspect_rules_ts//ts/private:ts_lib.bzl", "lib")

_EMPTY_DEPSET = depset()

# Output extension for each input extension.
# .ts/.tsx default to .js; explicit entries cover the module-type variants.
_EXT_MAP = {
    ".cts": ".cjs",
    ".cjs": ".cjs",
    ".mts": ".mjs",
    ".mjs": ".mjs",
}

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

    if path.endswith(".d.ts") or path.endswith(".d.mts") or path.endswith(".d.cts"):
        return None
    if path.endswith(".json"):
        return None

    ext_idx = path.rindex(".")
    src_ext = path[ext_idx:]

    if src_ext in (".js", ".jsx"):
        return _out_path(path, out_dir, root_dir) if allow_js else None

    out_ext = _EXT_MAP.get(src_ext, ".js")
    return _out_path(path[:ext_idx] + out_ext, out_dir, root_dir)

def _calculate_js_outs(srcs, allow_js, out_dir, root_dir):
    outs = []
    for src in srcs:
        out = _to_js_out(str(src), allow_js, out_dir, root_dir)
        if out:
            outs.append(out)
    return outs

def _js_transpiler_impl(ctx):
    outs = []  # JS files only, for JsInfo
    default_outs = []  # JS + map interleaved, for DefaultInfo
    transpile_srcs = []
    transpile_outs = []
    transpile_maps = []

    if ctx.attr.out_dir != "" and ctx.attr.root_dir == "":
        fail("When out_dir is set, root_dir must also be set.")

    src_paths = lib.files_relative_to_package(ctx, ctx.files.srcs)

    for src, src_path in zip(ctx.files.srcs, src_paths):
        # Type declaration files have no JS output.
        if src_path.endswith(".d.ts") or src_path.endswith(".d.mts") or src_path.endswith(".d.cts"):
            continue

        # JSON files are not transpiled.
        if src_path.endswith(".json"):
            continue

        root_dir = ctx.attr.root_dir.removesuffix("/")
        if root_dir == ".":
            root_dir = ""

        if root_dir != "" and not src_path.startswith(root_dir + "/"):
            fail(
                "Files to transpile must be in root_dir: \"{}\".\n".format(ctx.attr.root_dir) +
                "The file \"{}\" would produce output outside out_dir: \"{}\".\n".format(src_path, ctx.attr.out_dir),
            )

        ext_idx = src_path.rindex(".")
        src_ext = src_path[ext_idx:]

        # Plain JS/JSX files are copied through when allow_js is set.
        if src_ext in (".js", ".jsx"):
            if not ctx.attr.allow_js:
                continue
            out_path = lib.to_out_path(src_path, ctx.attr.out_dir, ctx.attr.root_dir)
            out = ctx.actions.declare_file(out_path)
            ctx.actions.run_shell(
                inputs = [src],
                outputs = [out],
                mnemonic = "CopyJs",
                command = "cp \"$1\" \"$2\"",
                arguments = [src.path, out.path],
                execution_requirements = {"supports-path-mapping": "1"},
            )
            outs.append(out)
            default_outs.append(out)
            continue

        out_ext = _EXT_MAP.get(src_ext, ".js")
        out_path = src_path[:ext_idx] + out_ext
        out_path = lib.to_out_path(out_path, ctx.attr.out_dir, ctx.attr.root_dir)
        out = ctx.actions.declare_file(out_path)
        outs.append(out)
        default_outs.append(out)
        transpile_srcs.append(src)
        transpile_outs.append(out)
        if ctx.attr.source_maps:
            map_out = ctx.actions.declare_file(out_path + ".map")
            transpile_maps.append(map_out)
            default_outs.append(map_out)

    if transpile_srcs:
        manifest_lines = []
        for src, out in zip(transpile_srcs, transpile_outs):
            manifest_lines.append(src.path)
            manifest_lines.append(out.path)
        manifest = ctx.actions.declare_file(ctx.label.name + "_manifest.txt")
        ctx.actions.write(manifest, "\n".join(manifest_lines) + "\n")

        args = ctx.actions.args()
        args.add("--mode")
        args.add("js")
        if ctx.attr.rewrite_extensions:
            args.add("--rewrite-extensions")
        if ctx.attr.source_maps:
            args.add("--source-maps")
        args.add("--manifest")
        args.add(manifest)

        ctx.actions.run(
            inputs = transpile_srcs + [manifest],
            arguments = [args],
            mnemonic = "TranspileJs",
            executable = ctx.executable._tool,
            outputs = transpile_outs + transpile_maps,
            execution_requirements = {"supports-path-mapping": "1"},
        )

    sources = depset(outs)

    return [
        JsInfo(
            target = ctx.label,
            sources = sources,
            types = _EMPTY_DEPSET,
            transitive_sources = _EMPTY_DEPSET,
            transitive_types = _EMPTY_DEPSET,
            npm_sources = _EMPTY_DEPSET,
            npm_package_store_infos = _EMPTY_DEPSET,
        ),
        DefaultInfo(
            files = depset(default_outs),
        ),
    ]

_js_transpiler_rule = rule(
    implementation = _js_transpiler_impl,
    attrs = {
        "srcs": attr.label_list(
            allow_files = True,
            default = [],
            doc = "TypeScript source files to transpile to JavaScript.",
        ),
        # Pre-declared at loading time so that labels like "dist/src/foo.js" in
        # entry_point attributes of downstream js_binary targets resolve to this
        # generated file rather than a source file that may exist on disk.
        "js_outs": attr.output_list(),
        "out_dir": attr.string(),
        "root_dir": attr.string(),
        "allow_js": attr.bool(default = False),
        "rewrite_extensions": attr.bool(
            default = False,
            doc = "Rewrite .ts/.mts/.cts import extensions to .js/.mjs/.cjs.",
        ),
        "source_maps": attr.bool(
            default = False,
            doc = "Emit a .js.map source map file alongside each output.",
        ),
        "_tool": attr.label(
            executable = True,
            default = "//build/rules/ts/oxc/transpiler",
            cfg = "exec",
        ),
    },
)

def js_transpiler(name, srcs, out_dir = "", root_dir = "", allow_js = False, rewrite_extensions = False, source_maps = False, **kwargs):
    """Macro wrapping _js_transpiler_rule that pre-declares output files at load time."""
    _js_transpiler_rule(
        name = name,
        srcs = srcs,
        js_outs = _calculate_js_outs(
            srcs,
            allow_js or False,
            out_dir or "",
            root_dir or "",
        ),
        out_dir = out_dir or "",
        root_dir = root_dir or "",
        allow_js = allow_js or False,
        rewrite_extensions = rewrite_extensions or False,
        source_maps = source_maps or False,
        **kwargs
    )
