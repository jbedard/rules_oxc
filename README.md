# Aspect's Bazel rules for oxc

`aspect_rules_oxc` is maintained by [Aspect Build](https://aspect.build).

## Installation

Requires Bazel 8.6 or greater with bzlmod (WORKSPACE is not supported).

Add to your `MODULE.bazel` file:

```starlark
bazel_dep(name = "aspect_rules_oxc", version = "0.0.0")
```

To use a commit rather than a release, use `archive_override` or `git_override` in
your `MODULE.bazel`.

## Limitations

### No `jsx=preserve` equivalent

JSX is always transformed. Preserved JSX can never run under Node, and a
JSX-aware bundler consumes the `.tsx` sources directly, so a preserve mode has
no coherent consumer here.

### Extension rewriting scope

Extension rewriting applies to any specifier containing a slash, including
bare package paths like `"pkg/util.ts"`, where tsc only rewrites relative ones.
