"""oxc_transpiler with a separate declaration_dir, like tsc's declarationDir.

The `transpiler` writes JS to out_dir while the `declaration_transpiler`
writes the .d.ts outputs to declaration_dir, matching the ts_project
declaration_dir attribute.
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
    declaration = True,
    declaration_dir = "dist/types",
    declaration_transpiler = partial.make(
        oxc_transpiler,
        declaration_dir = "dist/types",
        emit_dts = True,
        emit_js = False,
        root_dir = "src",
    ),
    out_dir = "dist/code",
    root_dir = "src",
    transpiler = partial.make(
        oxc_transpiler,
        out_dir = "dist/code",
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
    file1 = "dist/code/main.js",
    file2 = "snapshots/main.js",
)

diff_test(
    name = "dts_test",
    file1 = "dist/types/main.d.ts",
    file2 = "snapshots/main.d.ts",
)
