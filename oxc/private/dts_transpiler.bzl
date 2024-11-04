"""TypeScript declaration transpiler for ts_project."""

load("@aspect_rules_js//js:providers.bzl", "JsInfo")

_EMPTY_DEPSET = depset()

_EXT_MAP = {
    ".cjs": ".d.cts",
    ".cts": ".d.cts",
    ".mjs": ".d.mts",
    ".mts": ".d.mts",
}

def _dts_transpiler_impl(ctx):
    outs = []

    for src in ctx.files.srcs:
        filename = src.basename

        if filename.endswith(".json"):
            continue

        ext_idx = filename.rindex(".")
        out_filename = filename[:ext_idx] + _EXT_MAP.get(filename[ext_idx:], ".d.ts")

        out = ctx.actions.declare_file(out_filename, sibling = src)
        outs.append(out)

        args = ctx.actions.args()
        args.add(src)
        args.add(out)

        ctx.actions.run(
            inputs = [src],
            arguments = [args],
            mnemonic = "EmitDeclaration",
            executable = ctx.executable._tool,
            outputs = [out],
            execution_requirements = {
                "supports-path-mapping": "1",
            },
        )

    types = depset(outs)

    return [
        JsInfo(
            target = ctx.label,
            sources = _EMPTY_DEPSET,
            types = types,
            transitive_sources = _EMPTY_DEPSET,
            transitive_types = _EMPTY_DEPSET,
            npm_sources = _EMPTY_DEPSET,
            npm_package_store_infos = _EMPTY_DEPSET,
        ),
        DefaultInfo(
            files = types,
        ),
    ]

dts_transpiler = rule(
    implementation = _dts_transpiler_impl,
    attrs = {
        "srcs": attr.label_list(
            allow_files = True,
            default = [],
            doc = "Source files to be made available to dts",
        ),
	# TODO(zbarsky): Maybe turn this into a toolchain following the pattern in Aspect's bazel-lib
        "_tool": attr.label(
            executable = True,
            default = "//oxc/private:transpiler",
            cfg = "exec",
        ),
    },
)
