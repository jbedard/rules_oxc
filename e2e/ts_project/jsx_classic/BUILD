"""oxc_transpiler with the classic JSX runtime, matching tsconfig jsx=react.

tsc's jsx=react is spelled jsx="classic" on the transpiler: JSX compiles to
React.createElement calls. The React factory and JSX types come from the
ambient declarations in src/react.d.ts here, in place of the real package.
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
        "src/react.d.ts",
    ],
    declaration = True,
    out_dir = "dist",
    root_dir = "src",
    transpiler = partial.make(
        oxc_transpiler,
        jsx = "classic",
        out_dir = "dist",
        root_dir = "src",
    ),
    tsconfig = {
        "compilerOptions": {
            "jsx": "react",
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
