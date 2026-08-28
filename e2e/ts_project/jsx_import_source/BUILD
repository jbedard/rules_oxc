"""oxc_transpiler with a custom automatic JSX runtime, matching tsconfig
jsxImportSource.

The runtime import comes from "preact/jsx-runtime" instead of React's. The
module's types come from the ambient declaration in src/preact.d.ts here, in
place of the real package.
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
        "src/component.tsx",
        "src/preact.d.ts",
    ],
    declaration = True,
    out_dir = "dist",
    root_dir = "src",
    transpiler = partial.make(
        oxc_transpiler,
        jsx_import_source = "preact",
        out_dir = "dist",
        root_dir = "src",
    ),
    tsconfig = {
        "compilerOptions": {
            "jsx": "react-jsx",
            "jsxImportSource": "preact",
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
    file1 = "dist/component.js",
    file2 = "snapshots/component.js",
)

diff_test(
    name = "dts_test",
    file1 = "dist/component.d.ts",
    file2 = "snapshots/component.d.ts",
)
