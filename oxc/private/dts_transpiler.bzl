"""TypeScript declaration transpiler for ts_project."""

load("@aspect_rules_js//js:providers.bzl", "JsInfo")

# buildifier: disable=bzl-visibility
load("@aspect_rules_ts//ts/private:ts_lib.bzl", "lib")

_EMPTY_DEPSET = depset()

_EXT_MAP = {
    ".cjs": ".d.cts",
    ".cts": ".d.cts",
    ".mjs": ".d.mts",
    ".mts": ".d.mts",
}

def _dts_transpiler_impl(ctx):
    outs = []
    srcs = []

    if ctx.attr.out_dir != "" and ctx.attr.root_dir == "":
        fail("When out_dir is set, root_dir must also be set.")

    src_paths = lib.files_relative_to_package(ctx, ctx.files.srcs)
    for src, src_path in zip(ctx.files.srcs, src_paths):
        # {root_dir}/path/to/file.ts

        root_dir = ctx.attr.root_dir.removesuffix("/")
        if root_dir == ".":
            root_dir = ""

        if root_dir != "" and not src_path.startswith(root_dir + "/"):
            fail(
                "Files to create typings for must be in root_dir: \"{}\".\n".format(ctx.attr.root_dir) +
                "The file \"{}\" cannot have its typings generated because it would end up outside out_dir: \"{}\".\n\n".format(src_path, ctx.attr.out_dir),
            )

        if src_path.endswith(".json"):
            continue

        ext_idx = src_path.rindex(".")

        # {root_dir}/path/to/file.d.ts
        out_path = src_path[:ext_idx] + _EXT_MAP.get(src_path[ext_idx:], ".d.ts")

        # {out_dir}/path/to/file.d.ts
        out_path = lib.to_out_path(out_path, ctx.attr.out_dir, ctx.attr.root_dir)
        out = ctx.actions.declare_file(out_path)
        outs.append(out)
        srcs.append(src)

    if srcs:
        manifest_lines = []
        for src, out in zip(srcs, outs):
            manifest_lines.append(src.path)
            manifest_lines.append(out.path)
        manifest = ctx.actions.declare_file(ctx.label.name + "_manifest.txt")
        ctx.actions.write(manifest, "\n".join(manifest_lines) + "\n")

        args = ctx.actions.args()
        args.add("--mode")
        args.add("dts")
        args.add("--manifest")
        args.add(manifest)

        ctx.actions.run(
            inputs = srcs + [manifest],
            arguments = [args],
            mnemonic = "EmitDeclaration",
            executable = ctx.executable._tool,
            outputs = outs,
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
        "out_dir": attr.string(),
        "root_dir": attr.string(),
        "_tool": attr.label(
            executable = True,
            default = "//oxc/private:transpiler",
            cfg = "exec",
        ),
    },
)
