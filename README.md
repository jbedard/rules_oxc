# Aspect's Bazel rules for oxc

Bazel rules for [oxc](https://oxc.rs), a fast JavaScript/TypeScript toolchain
written in Rust; see the [oxc GitHub repository](https://github.com/oxc-project/oxc).

These rules run oxc's transpiler to convert TypeScript and JSX sources to
JavaScript, and can also emit `.d.ts` declarations via oxc's isolated
declarations support. Transpilation runs as a prebuilt native binary — no
Node.js toolchain involved. It can be used standalone, or configured as the
`ts_project` transpiler for
[rules_ts](https://github.com/aspect-build/rules_ts) so tsc typechecks while
oxc emits.

## More from Aspect

rules_oxc is just a part of what [Aspect Build](https://aspect.build) provides:

- _Need help?_ This ruleset has support provided by https://aspect.build/services.
- See our other Bazel rules, especially those built for rules_js, linked from
  https://github.com/aspect-build

## Installation

Requires Bazel 8.6 or greater with bzlmod (WORKSPACE is not supported).

Add a `bazel_dep` on `aspect_rules_oxc` to your `MODULE.bazel`, using the
snippet from the [latest release](https://github.com/aspect-build/rules_oxc/releases)
or the version on the
[Bazel Central Registry](https://registry.bazel.build/modules/aspect_rules_oxc).

To use a commit rather than a release, use `archive_override` or `git_override` in
your `MODULE.bazel`.

## Usage with rules_ts

Use `oxc_transpiler` as the `ts_project` transpiler to emit JavaScript with oxc
while tsc typechecks:

```starlark
load("@aspect_rules_oxc//oxc:defs.bzl", "oxc_transpiler")
load("@aspect_rules_ts//ts:defs.bzl", "ts_project")
load("@bazel_skylib//lib:partial.bzl", "partial")

ts_project(
    name = "lib",
    srcs = glob(["src/**/*.ts"]),
    out_dir = "dist",
    root_dir = "src",
    transpiler = partial.make(
        oxc_transpiler,
        out_dir = "dist",
        root_dir = "src",
    ),
)
```

`ts_project` does not forward `out_dir` and `root_dir` to the transpiler, so
they are repeated in `partial.make`.

Projects with `isolatedDeclarations` in their tsconfig can also set
`emit_dts = True` to emit the `.d.ts` outputs with oxc, replacing tsc's
declaration emit; see the
[ts_project e2e](e2e/ts_project/README.md) for that wiring and other
configurations.

## Performance

Each `oxc_transpiler` target transpiles all of its `srcs` in a single
multi-threaded action, rather than one action per file as `rules_swc` does.
Fewer actions means less scheduling and process-spawn overhead, at the cost of
coarser incrementality: changing one source re-transpiles every file in the
target.

The action reserves CPUs from Bazel's local scheduler via the `cpus` attribute.
By default it scales logarithmically with the number of `srcs` (2 threads at 3
files, 3 at 10, 4 at 100, capped at 4); set `cpus` explicitly to override.

## Supported platforms

Transpile actions run a prebuilt binary, so the *execution* platform (the
machine running Bazel actions, whether the local host or remote executors)
must be one of:

- Linux x86_64 or arm64
- macOS arm64

Intel macOS and Windows are not supported.

## Limitations

### No typechecking

Sources are transpiled independently without type information. Pair with a
typechecker such as `ts_project` (see above), which also surfaces oxc-invalid
code like non-isolated declarations.

### Declaration emit requires isolated declarations

`emit_dts` uses oxc's isolated declarations emit: every exported symbol needs
an explicit type annotation, and a non-inferable export is a build error where
tsc would infer the type. Set `isolatedDeclarations: true` in the tsconfig so
the typecheck enforces the same rules.

### No ESM-to-CommonJS transform

`module = "commonjs"` only transforms TypeScript's CommonJS-specific syntax
(`export =`, `import x = require(...)`); ESM import/export syntax in the
sources is an error. tsc's `module: commonjs` converts ESM syntax as well.

### No `es5` target

The lowest `target` is es2015: oxc cannot fully downlevel ES2015 syntax.

### Runtime helpers are always imported

Transforms that need a runtime helper import it from `@oxc-project/runtime`
(or `helpers_module`), which must then be a runtime dependency. There is no
inline-helper mode: behavior is always like tsc's `importHelpers: true`, with
oxc's runtime in place of `tslib`.

### No standard decorator transform

Only the legacy `experimental_decorators` transform is available. Standard
(TC39) decorators are emitted as written, so the runtime must support them.

### No `jsx=preserve` equivalent

JSX is always transformed. Preserved JSX can never run under Node, and a
JSX-aware bundler consumes the `.tsx` sources directly, so a preserve mode has
no coherent consumer here.

### Extension rewriting scope

Extension rewriting applies to any specifier containing a slash, including
bare package paths like `"pkg/util.ts"`, where tsc only rewrites relative ones.

# Telemetry & privacy policy

This ruleset collects limited usage data via [`tools_telemetry`](https://github.com/aspect-build/tools_telemetry), which is reported to Aspect Build Inc and governed by our [privacy policy](https://www.aspect.build/privacy-policy).
