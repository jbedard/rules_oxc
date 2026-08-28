"""oxc_transpiler with a custom classic JSX factory, matching tsconfig
jsxFactory and jsxFragmentFactory.

JSX compiles to h(...) calls with Fragment for fragments instead of
React.createElement and React.Fragment. Both come from the ambient
declarations in src/h.d.ts here, in place of a real package.
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
        "src/h.d.ts",
        "src/pair.tsx",
    ],
    declaration = True,
    out_dir = "dist",
    root_dir = "src",
    transpiler = partial.make(
        oxc_transpiler,
        jsx = "classic",
        jsx_factory = "h",
        jsx_fragment_factory = "Fragment",
        out_dir = "dist",
        root_dir = "src",
    ),
    tsconfig = {
        "compilerOptions": {
            "jsx": "react",
            "jsxFactory": "h",
            "jsxFragmentFactory": "Fragment",
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
    file1 = "dist/pair.js",
    file2 = "snapshots/pair.js",
)

diff_test(
    name = "dts_test",
    file1 = "dist/pair.d.ts",
    file2 = "snapshots/pair.d.ts",
)
