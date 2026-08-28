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
- `declaration_dir` - declarations written to a separate `declaration_dir`.
- `source_maps` - `source_map` mirrored by `source_maps` on the transpiler.
- `resolve_json_module` - `resolve_json_module` mirrored by `emit_json`.
- `target` - tsconfig `target` mirrored by `target`, downleveling the JS.
- `jsx_classic` - tsconfig `jsx: react` mirrored by `jsx = "classic"`.
- `rewrite_extensions` - tsconfig `rewriteRelativeImportExtensions` mirrored
  by `rewrite_extensions`.
- `module_commonjs` - tsconfig `module: commonjs` mirrored by
  `module = "commonjs"` for `.cts` sources.
- `use_define_for_class_fields` - tsconfig `useDefineForClassFields: false`
  mirrored by `use_define_for_class_fields = False`.
- `verbatim_module_syntax` - tsconfig `verbatimModuleSyntax` mirrored by
  `verbatim_module_syntax`, keeping imports unused after type stripping.
- `strip_internal` - tsconfig `stripInternal` mirrored by `strip_internal`,
  omitting `@internal` declarations from the shared transpile target's `.d.ts`.

It lives as a separate e2e workspace because `ts_project` requires the
`@npm_typescript` repository and node toolchain setup that the aspect_rules_oxc
module itself does not need.
