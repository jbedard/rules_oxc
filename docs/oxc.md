<!-- Generated with Stardoc: http://skydoc.bazel.build -->

Public API for the OXC transpiler rules.

<a id="oxc_transpiler"></a>

## oxc_transpiler

<pre>
load("@aspect_rules_oxc//oxc:defs.bzl", "oxc_transpiler")

oxc_transpiler(<a href="#oxc_transpiler-name">name</a>, <a href="#oxc_transpiler-srcs">srcs</a>, <a href="#oxc_transpiler-root_dir">root_dir</a>, <a href="#oxc_transpiler-out_dir">out_dir</a>, <a href="#oxc_transpiler-declaration_dir">declaration_dir</a>, <a href="#oxc_transpiler-cpus">cpus</a>, <a href="#oxc_transpiler-emit_js">emit_js</a>, <a href="#oxc_transpiler-emit_dts">emit_dts</a>, <a href="#oxc_transpiler-emit_json">emit_json</a>,
               <a href="#oxc_transpiler-source_maps">source_maps</a>, <a href="#oxc_transpiler-target">target</a>, <a href="#oxc_transpiler-module">module</a>, <a href="#oxc_transpiler-jsx">jsx</a>, <a href="#oxc_transpiler-rewrite_extensions">rewrite_extensions</a>, <a href="#oxc_transpiler-helpers_module">helpers_module</a>,
               <a href="#oxc_transpiler-use_define_for_class_fields">use_define_for_class_fields</a>, <a href="#oxc_transpiler-verbatim_module_syntax">verbatim_module_syntax</a>, <a href="#oxc_transpiler-strip_internal">strip_internal</a>, <a href="#oxc_transpiler-jsx_import_source">jsx_import_source</a>,
               <a href="#oxc_transpiler-jsx_factory">jsx_factory</a>, <a href="#oxc_transpiler-jsx_fragment_factory">jsx_fragment_factory</a>, <a href="#oxc_transpiler-experimental_decorators">experimental_decorators</a>, <a href="#oxc_transpiler-emit_decorator_metadata">emit_decorator_metadata</a>,
               <a href="#oxc_transpiler-strict_null_checks">strict_null_checks</a>, <a href="#oxc_transpiler-declaration_maps">declaration_maps</a>, <a href="#oxc_transpiler-inline_source_maps">inline_source_maps</a>, <a href="#oxc_transpiler-source_root">source_root</a>, <a href="#oxc_transpiler-remove_comments">remove_comments</a>,
               <a href="#oxc_transpiler-kwargs">**kwargs</a>)
</pre>

Transpile TypeScript and JavaScript sources with OXC.

Emits JavaScript and/or isolated .d.ts declarations without typechecking.
Each source file is transformed independently into its own outputs; nothing
is bundled. Provides JsInfo like ts_project, so targets can
consume the outputs interchangeably. Output files are pre-declared at load
time, letting downstream targets reference paths such as "dist/foo.js"
directly.


**PARAMETERS**


| Name  | Description | Default Value |
| :------------- | :------------- | :------------- |
| <a id="oxc_transpiler-name"></a>name |  target name.   |  none |
| <a id="oxc_transpiler-srcs"></a>srcs |  sources to transpile; files or targets carrying JsInfo.   |  none |
| <a id="oxc_transpiler-root_dir"></a>root_dir |  directory the srcs are relative to when computing output paths.   |  `""` |
| <a id="oxc_transpiler-out_dir"></a>out_dir |  directory for the JS outputs, relative to the package.   |  `""` |
| <a id="oxc_transpiler-declaration_dir"></a>declaration_dir |  directory for the declaration outputs, defaulting to out_dir.   |  `""` |
| <a id="oxc_transpiler-cpus"></a>cpus |  threads the transpile action may use, reserved via a cpu:<n> execution requirement. 0 (default) scales logarithmically with the number of srcs: one per decimal digit of the file count, plus one (2 at 3 files, 3 at 10), at most 4.   |  `0` |
| <a id="oxc_transpiler-emit_js"></a>emit_js |  emit JavaScript outputs.   |  `True` |
| <a id="oxc_transpiler-emit_dts"></a>emit_dts |  emit .d.ts declaration outputs.   |  `False` |
| <a id="oxc_transpiler-emit_json"></a>emit_json |  copy .json srcs into the output layout.   |  `False` |
| <a id="oxc_transpiler-source_maps"></a>source_maps |  emit a .map file alongside each JS output.   |  `False` |
| <a id="oxc_transpiler-target"></a>target |  ECMAScript target to downlevel the JS outputs to.   |  `""` |
| <a id="oxc_transpiler-module"></a>module |  module format of the JS outputs.   |  `""` |
| <a id="oxc_transpiler-jsx"></a>jsx |  JSX runtime, "automatic" or "classic".   |  `""` |
| <a id="oxc_transpiler-rewrite_extensions"></a>rewrite_extensions |  rewrite .ts-style import extensions to their JS extension.   |  `False` |
| <a id="oxc_transpiler-helpers_module"></a>helpers_module |  module to import runtime helpers from.   |  `""` |
| <a id="oxc_transpiler-use_define_for_class_fields"></a>use_define_for_class_fields |  class-field emit mode (tsc's useDefineForClassFields). Defaults like tsc to True for target es2022 and above (including the default, esnext) and False below. A select() must use the strings "true" and "false".   |  `None` |
| <a id="oxc_transpiler-verbatim_module_syntax"></a>verbatim_module_syntax |  keep imports that are unused after type stripping instead of eliding them (tsc's verbatimModuleSyntax).   |  `False` |
| <a id="oxc_transpiler-strip_internal"></a>strip_internal |  omit `/** @internal */` declarations from the .d.ts outputs (tsc's stripInternal).   |  `False` |
| <a id="oxc_transpiler-jsx_import_source"></a>jsx_import_source |  module the automatic JSX runtime is imported from (tsc's jsxImportSource).   |  `""` |
| <a id="oxc_transpiler-jsx_factory"></a>jsx_factory |  function the classic runtime compiles elements to (tsc's jsxFactory).   |  `""` |
| <a id="oxc_transpiler-jsx_fragment_factory"></a>jsx_fragment_factory |  expression the classic runtime compiles fragments to (tsc's jsxFragmentFactory).   |  `""` |
| <a id="oxc_transpiler-experimental_decorators"></a>experimental_decorators |  compile decorators with the legacy transform (tsc's experimentalDecorators).   |  `False` |
| <a id="oxc_transpiler-emit_decorator_metadata"></a>emit_decorator_metadata |  record design-time type metadata for decorated members (tsc's emitDecoratorMetadata).   |  `False` |
| <a id="oxc_transpiler-strict_null_checks"></a>strict_null_checks |  affects only decorator metadata; match the tsconfig's strictNullChecks.   |  `True` |
| <a id="oxc_transpiler-declaration_maps"></a>declaration_maps |  emit a .d.ts.map beside each declaration (tsc's declarationMap).   |  `False` |
| <a id="oxc_transpiler-inline_source_maps"></a>inline_source_maps |  embed the JS source maps as data URLs (tsc's inlineSourceMap).   |  `False` |
| <a id="oxc_transpiler-source_root"></a>source_root |  the sourceRoot recorded in every source map (tsc's sourceRoot).   |  `""` |
| <a id="oxc_transpiler-remove_comments"></a>remove_comments |  drop comments from the JS and declaration outputs (tsc's removeComments).   |  `False` |
| <a id="oxc_transpiler-kwargs"></a>kwargs |  common attributes forwarded to the rule.   |  none |


