"""oxc_transpiler with source maps, matching ts_project's source_map.

The tsconfig sourceMap option is mirrored by source_maps on the transpiler,
which writes a .map next to each JS output and appends the sourceMappingURL.
"""

load("@aspect_rules_oxc//oxc:defs.bzl", "oxc_transpiler")
load("@aspect_rules_ts//ts:defs.bzl", "ts_project")
load("@bazel_skylib//lib:partial.bzl", "partial")
load("@bazel_skylib//rules:build_test.bzl", "build_test")
load("@bazel_skylib//rules:diff_test.bzl", "diff_test")
load("@bazel_skylib//rules:select_file.bzl", "select_file")

package(default_testonly = True)

ts_project(
    name = "ts",
    srcs = ["src/main.ts"],
    declaration = True,
    out_dir = "dist",
    root_dir = "src",
    source_map = True,
    transpiler = partial.make(
        oxc_transpiler,
        out_dir = "dist",
        root_dir = "src",
        source_maps = True,
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

# The .map is not pre-declared, so it is selected from the transpile target.
select_file(
    name = "map",
    srcs = ":ts_transpile",
    subpath = "dist/main.js.map",
)

diff_test(
    name = "map_test",
    file1 = ":map",
    file2 = "snapshots/main.js.map",
)
