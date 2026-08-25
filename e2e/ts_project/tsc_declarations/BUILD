"""oxc_transpiler for JS with tsc emitting the declaration outputs.

Only `transpiler` is customized; declarations come from tsc itself, useful for
sources that don't satisfy isolated declarations rules.
"""

load("@aspect_rules_ts//ts:defs.bzl", "ts_project")
load("@bazel_skylib//lib:partial.bzl", "partial")
load("@bazel_skylib//rules:build_test.bzl", "build_test")
load("@bazel_skylib//rules:diff_test.bzl", "diff_test")
load("@rules_oxc//oxc:defs.bzl", "oxc_transpiler")

package(default_testonly = True)

ts_project(
    name = "ts",
    srcs = ["src/main.ts"],
    declaration = True,
    out_dir = "dist",
    root_dir = "src",
    transpiler = partial.make(
        oxc_transpiler,
        out_dir = "dist",
        root_dir = "src",
    ),
    tsconfig = {},
)

build_test(
    name = "ts_project_test",
    targets = [
        ":ts",
        ":ts_types",
    ],
)

diff_test(
    name = "js_test",
    file1 = "dist/main.js",
    file2 = "snapshots/main.js",
)

diff_test(
    name = "dts_test",
    file1 = "dist/main.d.ts",
    file2 = "snapshots/main.d.ts",
)
