"""oxc_transpiler emitting CommonJS-specific syntax, matching tsconfig
module=commonjs.

`export =` and `import x = require(...)` in .cts sources are rewritten to
module.exports and require calls; ESM syntax would be an error.
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
        "src/answer.cts",
        "src/api.cts",
    ],
    declaration = True,
    out_dir = "dist",
    root_dir = "src",
    transpiler = partial.make(
        oxc_transpiler,
        module = "commonjs",
        out_dir = "dist",
        root_dir = "src",
    ),
    tsconfig = {
        "compilerOptions": {
            "module": "commonjs",
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
    file1 = "dist/api.cjs",
    file2 = "snapshots/api.cjs",
)

diff_test(
    name = "dts_test",
    file1 = "dist/api.d.cts",
    file2 = "snapshots/api.d.cts",
)
