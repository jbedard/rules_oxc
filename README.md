# Template for Bazel rules

Copy this template to create a Bazel ruleset.

Features:

- follows the official style guide at https://bazel.build/rules/deploying
- bzlmod (MODULE.bazel) only; requires Bazel 8.6 or greater
- includes Bazel formatting as a pre-commit hook (using [buildifier])
- includes typical toolchain setup
- CI configured with GitHub Actions
- release using GitHub Actions just by pushing a tag
- the release artifact doesn't need to be built by Bazel, but can still exclude files and stamp the version

Ready to get started? Copy this repo, then

1. search for "com_myorg_rules_mylang" and replace with the name you'll use for your workspace
1. search for "myorg" and replace with GitHub org
1. search for "mylang", "Mylang", "MYLANG" and replace with the language/tool your rules are for
1. rename directory "mylang" similarly
1. run `pre-commit install` to get lints (see CONTRIBUTING.md)
1. if you don't need to fetch platform-dependent tools, then remove anything toolchain-related.
1. (optional) install the [Renovate app](https://github.com/apps/renovate) to get auto-PRs to keep the dependencies up-to-date.
1. delete this section of the README (everything up to the SNIP).

Optional: if you write tools for your rules to call, you should avoid toolchain dependencies for those tools leaking to all users.
For example, https://github.com/aspect-build/rules_py actions rely on a couple of binaries written in Rust, but we don't want users to be forced to
fetch a working Rust toolchain. Instead we want to ship pre-built binaries on our GH releases, and the ruleset fetches these as toolchains.
See https://blog.aspect.build/releasing-bazel-rulesets-rust for information on how to do this.
Note that users who *do* want to build tools from source should still be able to do so, they just need to register a different toolchain earlier.

---- SNIP ----

# Bazel rules for mylang

## Installation

Requires Bazel 8.6 or greater with bzlmod (WORKSPACE is not supported).

Add to your `MODULE.bazel` file:

```starlark
bazel_dep(name = "rules_oxc", version = "0.0.0")
```

To use a commit rather than a release, use `archive_override` or `git_override` in
your `MODULE.bazel`.
