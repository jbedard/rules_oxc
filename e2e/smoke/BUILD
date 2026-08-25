"""Exercises rules_oxc from an end-user's perspective."""

load("@bazel_skylib//rules:diff_test.bzl", "diff_test")
load("@rules_oxc//oxc:defs.bzl", "dts_transpiler", "js_transpiler")

js_transpiler(
    name = "js",
    srcs = ["src/main.ts"],
    out_dir = "dist",
    root_dir = "src",
)

dts_transpiler(
    name = "dts",
    srcs = ["src/main.ts"],
    out_dir = "dist",
    root_dir = "src",
)

diff_test(
    name = "js_test",
    file1 = "dist/main.js",
    file2 = "golden/main.js.golden",
)

diff_test(
    name = "dts_test",
    file1 = ":dts",
    file2 = "golden/main.d.ts.golden",
)
