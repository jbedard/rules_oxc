"""Public API for the OXC transpiler rules."""

load("@aspect_tools_telemetry_report//:defs.bzl", "TELEMETRY")  # buildifier: disable=load
load("//oxc/private:oxc_transpiler.bzl", _oxc_transpiler = "oxc_transpiler")

oxc_transpiler = _oxc_transpiler
