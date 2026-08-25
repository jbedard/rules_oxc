"""Single shared oxc_transpiler invocation with isolatedDeclarations enforced.

oxc declaration emit only supports sources that satisfy TypeScript's isolated
declarations rules. Setting isolatedDeclarations in the tsconfig makes the tsc
typecheck enforce those rules, so sources that oxc cannot emit declarations
for fail the typecheck instead of producing wrong output.
"""

load("@aspect_rules_ts//ts:defs.bzl", "ts_project")
load("@bazel_skylib//lib:partial.bzl", "partial")
load("@bazel_skylib//rules:build_test.bzl", "build_test")
load("@bazel_skylib//rules:diff_test.bzl", "diff_test")
load("@rules_oxc//oxc:defs.bzl", "oxc_transpiler")
load("//:single_transpiler.bzl", "oxc_transpiler_declarations")

package(default_testonly = True)

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
    tsconfig = {
        "compilerOptions": {
            "declaration": True,
            "isolatedDeclarations": True,
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
