"""Exercises oxc_transpiler as the ts_project transpiler + declaration_transpiler."""

load("@aspect_rules_oxc//oxc:defs.bzl", "oxc_transpiler")
load("@aspect_rules_ts//ts:defs.bzl", "ts_project")
load("@bazel_skylib//lib:partial.bzl", "partial")
load("@bazel_skylib//rules:build_test.bzl", "build_test")
load("@bazel_skylib//rules:diff_test.bzl", "diff_test")
load("//:single_transpiler.bzl", "oxc_transpiler_declarations")

package(default_testonly = True)

# ts_project wired so that a single oxc_transpiler invocation (one OxcTranspile
# action on the "ts_transpile" target) produces both the JS and declaration
# outputs: the `transpiler` emits declarations too (emit_dts = True), and the
# `declaration_transpiler` only re-exposes those declarations from the shared
# target's "types" output group without creating another transpile action.
ts_project(
    name = "ts",
    srcs = ["src/main.ts"],
    declaration = True,
    declaration_transpiler = oxc_transpiler_declarations,
    out_dir = "dist",
    root_dir = "src",
    transpiler = partial.make(
        oxc_transpiler,
        emit_dts = True,
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

# Both outputs come from the single shared transpile action.
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
