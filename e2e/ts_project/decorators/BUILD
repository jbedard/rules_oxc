"""oxc_transpiler with legacy decorators, matching tsconfig experimentalDecorators
and emitDecoratorMetadata.

Decorators compile to _decorate helper calls with design:* metadata, as tsc
emits them; the helpers come from @oxc-project/runtime and Reflect.metadata
from a reflect-metadata polyfill, both runtime dependencies of the output.
"""

load("@aspect_rules_oxc//oxc:defs.bzl", "oxc_transpiler")
load("@aspect_rules_ts//ts:defs.bzl", "ts_project")
load("@bazel_skylib//lib:partial.bzl", "partial")
load("@bazel_skylib//rules:build_test.bzl", "build_test")
load("@bazel_skylib//rules:diff_test.bzl", "diff_test")

package(default_testonly = True)

ts_project(
    name = "ts",
    srcs = ["src/main.ts"],
    declaration = True,
    out_dir = "dist",
    root_dir = "src",
    transpiler = partial.make(
        oxc_transpiler,
        emit_decorator_metadata = True,
        experimental_decorators = True,
        out_dir = "dist",
        root_dir = "src",
    ),
    tsconfig = {
        "compilerOptions": {
            "emitDecoratorMetadata": True,
            "experimentalDecorators": True,
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
