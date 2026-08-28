"""oxc_transpiler rewriting .ts import extensions, matching
tsconfig rewriteRelativeImportExtensions.

Sources import each other with their .ts extension; the transpiler rewrites
those specifiers to .js in the outputs.
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
        "src/main.ts",
        "src/util.ts",
    ],
    declaration = True,
    out_dir = "dist",
    root_dir = "src",
    transpiler = partial.make(
        oxc_transpiler,
        out_dir = "dist",
        rewrite_extensions = True,
        root_dir = "src",
    ),
    tsconfig = {
        "compilerOptions": {
            "rewriteRelativeImportExtensions": True,
        },
    },
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
