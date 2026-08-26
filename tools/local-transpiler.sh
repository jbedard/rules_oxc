#!/usr/bin/env bash
# Build the transpiler from its isolated module and stage the binary for
# --config=local-transpiler (see .bazelrc). Run from anywhere in the repo:
#   tools/local-transpiler.sh
#   bazel test //oxc/... --config=local-transpiler
set -euo pipefail

cd "$(dirname "$0")/.."

(cd transpiler && bazel build //:transpiler)

cp -f transpiler/bazel-bin/transpiler tools/transpiler-override/file/transpiler
chmod u+w tools/transpiler-override/file/transpiler

echo "Staged locally built transpiler at tools/transpiler-override/file/transpiler"
echo "Run tests with: bazel test //oxc/... --config=local-transpiler"
