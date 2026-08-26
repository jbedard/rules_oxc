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
