"""Public API for the OXC transpiler rules."""

load("//oxc/private:dts_transpiler.bzl", _dts_transpiler = "dts_transpiler")
load("//oxc/private:js_transpiler.bzl", _js_transpiler = "js_transpiler")

dts_transpiler = _dts_transpiler
js_transpiler = _js_transpiler
