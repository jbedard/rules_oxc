# ts_project test

This e2e exercises using `oxc_transpiler` with the `ts_project` `transpiler`
and `declaration_transpiler` attributes, one configuration per subdirectory:

- `single_invocation` - a single `oxc_transpiler` invocation (one
  `OxcTranspile` action) produces both the JavaScript and declaration outputs.
- `isolated_declarations` - same wiring, with `isolatedDeclarations` set in
  the tsconfig so the tsc typecheck enforces the rules oxc declaration emit
  relies on.
- `js_only` - oxc produces only JavaScript; no declaration outputs.
- `declarations_only` - `emit_declaration_only` with oxc producing only
  declaration outputs.
- `tsc_declarations` - oxc produces JavaScript while tsc emits declarations.

It lives as a separate e2e workspace because `ts_project` requires the
`@npm_typescript` repository and node toolchain setup that the rules_oxc
module itself does not need.
