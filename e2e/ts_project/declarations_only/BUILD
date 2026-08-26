"""oxc_transpiler as the declaration transpiler only: emit_declaration_only.

With emit_declaration_only, ts_project only invokes `declaration_transpiler`;
oxc_transpiler is configured with emit_js = False so its single action emits
just the .d.ts outputs.
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
    declaration_transpiler = partial.make(
        oxc_transpiler,
        emit_dts = True,
        emit_js = False,
        out_dir = "dist",
        root_dir = "src",
    ),
    emit_declaration_only = True,
    out_dir = "dist",
    root_dir = "src",
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
    name = "dts_test",
    file1 = "dist/main.d.ts",
    file2 = "snapshots/main.d.ts",
)
