# Bazel rules for oxc

## Installation

Requires Bazel 8.6 or greater with bzlmod (WORKSPACE is not supported).

Add to your `MODULE.bazel` file:

```starlark
bazel_dep(name = "rules_oxc", version = "0.0.0")
```

To use a commit rather than a release, use `archive_override` or `git_override` in
your `MODULE.bazel`.
