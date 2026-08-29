"""oxc_transpiler emitting declaration maps, matching ts_project's declaration_map.

The single shared transpile target emits the JS, the declarations and,
with declaration_maps, a .d.ts.map beside each declaration, re-exposed
through the "types" output group like the declarations themselves.
"""

load("@aspect_rules_oxc//oxc:defs.bzl", "oxc_transpiler")
load("@aspect_rules_ts//ts:defs.bzl", "ts_project")
load("@bazel_skylib//lib:partial.bzl", "partial")
load("@bazel_skylib//rules:build_test.bzl", "build_test")
load("@bazel_skylib//rules:diff_test.bzl", "diff_test")
load("@bazel_skylib//rules:select_file.bzl", "select_file")
load("//:single_transpiler.bzl", "oxc_transpiler_declarations")

package(default_testonly = True)

ts_project(
    name = "ts",
    srcs = ["src/main.ts"],
    declaration = True,
    declaration_map = True,
    declaration_transpiler = oxc_transpiler_declarations,
    out_dir = "dist",
    root_dir = "src",
    transpiler = partial.make(
        oxc_transpiler,
        declaration_maps = True,
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

diff_test(
    name = "dts_test",
    file1 = "dist/main.d.ts",
    file2 = "snapshots/main.d.ts",
)

# The map is not pre-declared, so it is selected from the types target.
select_file(
    name = "map",
    srcs = ":ts_types",
    subpath = "dist/main.d.ts.map",
)

diff_test(
    name = "map_test",
    file1 = ":map",
    file2 = "snapshots/main.d.ts.map",
)
