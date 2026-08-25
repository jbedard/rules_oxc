"""Helpers to share one oxc_transpiler invocation between the ts_project
`transpiler` and `declaration_transpiler` attributes.

ts_project invokes `transpiler` to create "{name}_transpile" and
`declaration_transpiler` to create "{name}_declarations". By configuring the
`transpiler` oxc_transpiler with emit_dts = True, the single "{name}_transpile"
target (and its single OxcTranspile action) produces both the JS and the .d.ts
outputs. The declaration transpiler then only needs to re-expose the .d.ts
files via the "types" output group of that shared target, creating no
additional transpile action.
"""

def oxc_transpiler_declarations(name, srcs, **kwargs):
    """ts_project declaration_transpiler that reuses the sibling "_transpile" target.

    Args:
        name: target name, always "{ts_project_name}_declarations".
        srcs: ignored; the declarations come from the shared transpile target.
        **kwargs: common attributes forwarded by ts_project.
    """

    # srcs intentionally unused: outputs come from the shared transpile action.
    _ = srcs  # @unused

    native.filegroup(
        name = name,
        srcs = [name.removesuffix("_declarations") + "_transpile"],
        output_group = "types",
        **kwargs
    )
