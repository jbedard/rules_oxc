"""oxc_transpiler embedding source maps, matching tsconfig inlineSourceMap and
sourceRoot.

The map is carried in each JS output's sourceMappingURL data URL instead of
a .js.map file, with sourceRoot recorded in it.
"""

load("@aspect_rules_oxc//oxc:defs.bzl", "oxc_transpiler")
load("@aspect_rules_ts//ts:defs.bzl", "ts_project")
load("@bazel_skylib//lib:partial.bzl", "partial")
load("@bazel_skylib//rules:build_test.bzl", "build_test")
load("@bazel_skylib//rules:diff_test.bzl", "diff_test")

package(default_testonly = True)

ts_project(
    name = "ts",
    srcs = ["src/main.ts"],
    out_dir = "dist",
    root_dir = "src",
    transpiler = partial.make(
        oxc_transpiler,
        inline_source_maps = True,
        out_dir = "dist",
        root_dir = "src",
        source_root = "/src",
    ),
    tsconfig = {
        "compilerOptions": {
            "inlineSourceMap": True,
            "sourceRoot": "/src",
        },
    },
)

build_test(
    name = "ts_project_test",
    targets = [":ts"],
)

diff_test(
    name = "js_test",
    file1 = "dist/main.js",
    file2 = "snapshots/main.js",
)
