"""Exercises rules_oxc from an end-user's perspective."""

load("@bazel_skylib//rules:build_test.bzl", "build_test")
load("@bazel_skylib//rules:diff_test.bzl", "diff_test")
load("@rules_oxc//oxc:defs.bzl", "oxc_transpiler")

package(default_testonly = True)

oxc_transpiler(
    name = "transpile",
    srcs = ["src/main.ts"],
    emit_dts = True,
    out_dir = "dist",
    root_dir = "src",
)

# Both .js and .d.ts outputs are pre-declared at load time, so they are
# addressable as labels by downstream targets.
build_test(
    name = "predeclared_outputs_test",
    targets = [
        "dist/main.js",
        "dist/main.d.ts",
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
