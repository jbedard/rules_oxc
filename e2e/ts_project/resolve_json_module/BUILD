"""oxc_transpiler copying json srcs, matching ts_project's resolve_json_module.

The tsconfig resolveJsonModule option is mirrored by emit_json on the
transpiler, which copies each json src into the output layout like tsc.
"""

load("@aspect_rules_oxc//oxc:defs.bzl", "oxc_transpiler")
load("@aspect_rules_ts//ts:defs.bzl", "ts_project")
load("@bazel_skylib//lib:partial.bzl", "partial")
load("@bazel_skylib//rules:build_test.bzl", "build_test")
load("@bazel_skylib//rules:diff_test.bzl", "diff_test")

package(default_testonly = True)

ts_project(
    name = "ts",
    srcs = [
        "src/data.json",
        "src/main.ts",
    ],
    out_dir = "dist",
    resolve_json_module = True,
    root_dir = "src",
    transpiler = partial.make(
        oxc_transpiler,
        emit_json = True,
        out_dir = "dist",
        root_dir = "src",
    ),
    tsconfig = {},
)

build_test(
    name = "ts_project_test",
    targets = [
        ":ts",
        "dist/data.json",
    ],
)

diff_test(
    name = "js_test",
    file1 = "dist/main.js",
    file2 = "snapshots/main.js",
)

diff_test(
    name = "json_test",
    file1 = "dist/data.json",
    file2 = "src/data.json",
)
