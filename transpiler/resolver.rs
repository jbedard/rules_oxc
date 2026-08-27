// Resolution of extensionless relative import specifiers against source files on disk, with
// tsc rootDirs-style overlaying of multiple root directories.

use std::path::{Path, PathBuf};

// TypeScript sources first: with both foo.ts and foo.js present, tsc resolves "./foo" to foo.ts.
const RESOLVABLE_EXTS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

fn js_ext_for(src_ext: &str) -> &'static str {
    match src_ext {
        "mts" | "mjs" => "mjs",
        "cts" | "cjs" => "cjs",
        _ => "js",
    }
}

// Declaration-only targets resolve like tsc: importing "./gen" where only gen.d.ts exists emits
// "./gen.js", assuming the implementation exists at runtime.
const DECLARATION_SUFFIXES: &[(&str, &str)] =
    &[(".d.ts", "js"), (".d.mts", "mjs"), (".d.cts", "cjs")];

// Lexically fold "." and ".." components so root prefixes match without filesystem access.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// Paths to probe for a relative specifier: the normalized import target first, then — when it lies
// under a root_dir, taking the longest matching root like tsc — the same relative location under each
// other root, mirroring how tsc's rootDirs overlays the roots into one virtual directory. Matching the
// target rather than the importer's directory keeps "../" specifiers that leave an overlapping root resolvable.
fn candidate_targets(base_dir: &Path, specifier: &str, root_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let target = normalize(&base_dir.join(specifier));
    let mut targets = vec![target.clone()];

    // Roots are normalized so lexical forms like "./app" still prefix-match the normalized target.
    let root_dirs: Vec<PathBuf> = root_dirs.iter().map(|root| normalize(root)).collect();
    let longest = root_dirs
        .iter()
        .filter_map(|root| target.strip_prefix(root).ok().map(|rel| (root, rel.to_path_buf())))
        .max_by_key(|(root, _)| root.components().count());

    if let Some((longest_root, rel)) = longest {
        for other in &root_dirs {
            if other != longest_root {
                let candidate = other.join(&rel);
                if !targets.contains(&candidate) {
                    targets.push(candidate);
                }
            }
        }
    }
    targets
}

pub(crate) fn resolve_specifier(
    base_dir: &Path,
    specifier: &str,
    root_dirs: &[PathBuf],
) -> Option<String> {
    if !specifier.starts_with("./")
        && !specifier.starts_with("../")
        && specifier != "."
        && specifier != ".."
    {
        return None;
    }

    // A specifier carrying a JS or TS extension is fully specified; rewriting TS extensions is
    // the transformer's job. Any other dot is part of the name (./foo.service, ./v1.2) and the
    // specifier is resolved like tsc.
    if let Some(ext) = Path::new(specifier).extension().and_then(|e| e.to_str())
        && RESOLVABLE_EXTS.contains(&ext)
    {
        return None;
    }

    // A specifier naming a directory (trailing slash, or a "." / ".." final component) resolves
    // only via its index file, never via a sibling file of the same name.
    let trimmed = specifier.trim_end_matches('/');
    let dir_only = trimmed.len() != specifier.len()
        || matches!(trimmed.rsplit('/').next(), Some(".") | Some(".."));

    let has_extension = Path::new(trimmed).extension().is_some();

    // Each candidate target is resolved completely (file, declaration, then index file) before moving
    // to the next, matching tsc: a directory's own index file wins over a file in a later root.
    for target in candidate_targets(base_dir, trimmed, root_dirs) {
        if !dir_only {
            // An extensioned specifier naming an existing file (e.g. a ./data.json asset copied
            // through to the output) is already runtime-valid: it wins over a dotted source like
            // data.json.ts and is left unchanged.
            if has_extension && target.is_file() {
                return None;
            }
            if let Some(js_ext) = probe(&target) {
                return Some(format!("{trimmed}.{js_ext}"));
            }
        }
        if let Some(js_ext) = probe(&target.join("index")) {
            return Some(format!("{trimmed}/index.{js_ext}"));
        }
    }

    None
}

// `target` with `suffix` appended to the file name. Path::with_extension would instead replace
// anything after a dot already in the name (foo.service -> foo.ts).
fn with_suffix(target: &Path, suffix: &str) -> PathBuf {
    let mut s = target.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

// The emitted JS extension for a source or declaration file existing at `target` with any
// resolvable extension appended, or None when no such file exists.
fn probe(target: &Path) -> Option<&'static str> {
    for &ext in RESOLVABLE_EXTS {
        if with_suffix(target, &format!(".{ext}")).is_file() {
            return Some(js_ext_for(ext));
        }
    }

    for &(suffix, js_ext) in DECLARATION_SUFFIXES {
        if with_suffix(target, suffix).is_file() {
            return Some(js_ext);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::test_dir;
    use std::fs;

    #[test]
    fn js_ext_mapping() {
        assert_eq!(js_ext_for("ts"), "js");
        assert_eq!(js_ext_for("tsx"), "js");
        assert_eq!(js_ext_for("mts"), "mjs");
        assert_eq!(js_ext_for("cts"), "cjs");
        assert_eq!(js_ext_for("js"), "js");
        assert_eq!(js_ext_for("jsx"), "js");
        assert_eq!(js_ext_for("mjs"), "mjs");
        assert_eq!(js_ext_for("cjs"), "cjs");
    }

    #[test]
    fn resolve_specifier_ignores_bare_and_extensioned() {
        let dir = test_dir("resolve_ignores");
        assert_eq!(resolve_specifier(&dir, "lodash", &[]), None);
        assert_eq!(resolve_specifier(&dir, "./b.js", &[]), None);
        assert_eq!(resolve_specifier(&dir, "./b.ts", &[]), None);
    }

    #[test]
    fn resolve_specifier_resolves_sibling_file() {
        let dir = test_dir("resolve_sibling");
        fs::write(dir.join("b.ts"), "").unwrap();
        fs::write(dir.join("c.mts"), "").unwrap();
        fs::write(dir.join("d.jsx"), "").unwrap();
        fs::write(dir.join("e.cjs"), "").unwrap();
        assert_eq!(resolve_specifier(&dir, "./b", &[]), Some("./b.js".to_string()));
        assert_eq!(resolve_specifier(&dir, "./c", &[]), Some("./c.mjs".to_string()));
        assert_eq!(resolve_specifier(&dir, "./d", &[]), Some("./d.js".to_string()));
        assert_eq!(resolve_specifier(&dir, "./e", &[]), Some("./e.cjs".to_string()));
        assert_eq!(resolve_specifier(&dir, "./missing", &[]), None);
    }

    #[test]
    fn resolve_specifier_prefers_ts_over_js() {
        let dir = test_dir("resolve_prefers_ts");
        fs::write(dir.join("b.mts"), "").unwrap();
        fs::write(dir.join("b.js"), "").unwrap();
        assert_eq!(resolve_specifier(&dir, "./b", &[]), Some("./b.mjs".to_string()));
    }

    // Dots in a file name are part of the name, not an extension: the specifier resolves like
    // tsc, and the JS extension is appended rather than replacing the last dotted segment.
    #[test]
    fn resolve_specifier_resolves_dotted_names() {
        let dir = test_dir("resolve_dotted");
        fs::write(dir.join("foo.service.ts"), "").unwrap();
        fs::write(dir.join("v1.2.mts"), "").unwrap();
        fs::write(dir.join("gen.v1.d.ts"), "").unwrap();
        fs::write(dir.join("data.json.ts"), "").unwrap();
        assert_eq!(
            resolve_specifier(&dir, "./foo.service", &[]),
            Some("./foo.service.js".to_string())
        );
        assert_eq!(resolve_specifier(&dir, "./v1.2", &[]), Some("./v1.2.mjs".to_string()));
        assert_eq!(resolve_specifier(&dir, "./gen.v1", &[]), Some("./gen.v1.js".to_string()));
        assert_eq!(
            resolve_specifier(&dir, "./data.json", &[]),
            Some("./data.json.js".to_string())
        );
        assert_eq!(resolve_specifier(&dir, "./missing.name", &[]), None);
    }

    // An asset existing at the exact specifier wins over a dotted source with an appended
    // extension: the specifier is already runtime-valid and left unchanged.
    #[test]
    fn resolve_specifier_prefers_exact_file_over_dotted_source() {
        let dir = test_dir("resolve_exact_asset");
        fs::write(dir.join("data.json"), "").unwrap();
        fs::write(dir.join("data.json.ts"), "").unwrap();
        assert_eq!(resolve_specifier(&dir, "./data.json", &[]), None);
    }

    #[test]
    fn resolve_specifier_resolves_dot_and_parent() {
        let dir = test_dir("resolve_dot_specifiers");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("index.ts"), "").unwrap();
        assert_eq!(resolve_specifier(&dir, ".", &[]), Some("./index.js".to_string()));
        assert_eq!(
            resolve_specifier(&dir.join("sub"), "..", &[]),
            Some("../index.js".to_string())
        );
    }

    // A "." specifier names the importing directory itself: a sibling file matching the
    // directory's name must not resolve.
    #[test]
    fn resolve_specifier_dot_ignores_sibling_file() {
        let dir = test_dir("resolve_dot_sibling");
        fs::create_dir_all(dir.join("app")).unwrap();
        fs::write(dir.join("app.ts"), "").unwrap();
        assert_eq!(resolve_specifier(&dir.join("app"), ".", &[]), None);
    }

    #[test]
    fn resolve_specifier_trailing_slash_resolves_index_only() {
        let dir = test_dir("resolve_trailing_slash");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/index.ts"), "").unwrap();
        fs::write(dir.join("other.ts"), "").unwrap();
        assert_eq!(
            resolve_specifier(&dir, "./sub/", &[]),
            Some("./sub/index.js".to_string())
        );
        assert_eq!(resolve_specifier(&dir, "./other/", &[]), None);
    }

    #[test]
    fn candidate_targets_normalizes_roots() {
        let roots = [PathBuf::from("./app"), PathBuf::from("lib/")];
        let targets = candidate_targets(Path::new("app"), "./x", &roots);
        assert_eq!(targets, vec![PathBuf::from("app/x"), PathBuf::from("lib/x")]);
    }

    #[test]
    fn resolve_specifier_resolves_directory_index() {
        let dir = test_dir("resolve_index");
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/index.ts"), "").unwrap();
        assert_eq!(
            resolve_specifier(&dir, "./sub", &[]),
            Some("./sub/index.js".to_string())
        );
    }

    #[test]
    fn resolve_specifier_prefers_file_over_index() {
        let dir = test_dir("resolve_prefers_file");
        fs::write(dir.join("b.ts"), "").unwrap();
        fs::create_dir_all(dir.join("b")).unwrap();
        fs::write(dir.join("b/index.ts"), "").unwrap();
        assert_eq!(resolve_specifier(&dir, "./b", &[]), Some("./b.js".to_string()));
    }

    #[test]
    fn resolve_specifier_across_root_dirs() {
        let dir = test_dir("resolve_root_dirs");
        fs::create_dir_all(dir.join("app")).unwrap();
        fs::create_dir_all(dir.join("lib/sub")).unwrap();
        fs::write(dir.join("lib/shared.ts"), "").unwrap();
        fs::write(dir.join("lib/gen.d.ts"), "").unwrap();
        fs::write(dir.join("lib/genm.d.mts"), "").unwrap();
        fs::write(dir.join("lib/sub/index.ts"), "").unwrap();
        let roots = [dir.join("app"), dir.join("lib")];
        let base = dir.join("app");
        assert_eq!(
            resolve_specifier(&base, "./shared", &roots),
            Some("./shared.js".to_string())
        );
        assert_eq!(
            resolve_specifier(&base, "./gen", &roots),
            Some("./gen.js".to_string())
        );
        assert_eq!(
            resolve_specifier(&base, "./genm", &roots),
            Some("./genm.mjs".to_string())
        );
        assert_eq!(
            resolve_specifier(&base, "./sub", &roots),
            Some("./sub/index.js".to_string())
        );
        assert_eq!(resolve_specifier(&base, "./missing", &roots), None);
    }

    #[test]
    fn resolve_specifier_across_root_dirs_subdir() {
        let dir = test_dir("resolve_root_dirs_subdir");
        fs::create_dir_all(dir.join("app/deep")).unwrap();
        fs::create_dir_all(dir.join("lib/deep")).unwrap();
        fs::write(dir.join("lib/deep/util.ts"), "").unwrap();
        let roots = [dir.join("app"), dir.join("lib")];
        assert_eq!(
            resolve_specifier(&dir.join("app/deep"), "./util", &roots),
            Some("./util.js".to_string())
        );
    }

    #[test]
    fn resolve_specifier_prefers_own_dir_over_other_roots() {
        let dir = test_dir("resolve_root_dirs_own_dir");
        fs::create_dir_all(dir.join("app")).unwrap();
        fs::create_dir_all(dir.join("lib")).unwrap();
        fs::write(dir.join("app/x.ts"), "").unwrap();
        fs::write(dir.join("lib/x.mts"), "").unwrap();
        let roots = [dir.join("app"), dir.join("lib")];
        assert_eq!(
            resolve_specifier(&dir.join("app"), "./x", &roots),
            Some("./x.js".to_string())
        );
    }

    // A directory's own index file wins over a file at the same specifier in a later root: each
    // candidate dir is resolved completely before moving on, matching tsc.
    #[test]
    fn resolve_specifier_own_index_beats_other_root_file() {
        let dir = test_dir("resolve_root_dirs_index_first");
        fs::create_dir_all(dir.join("app/x")).unwrap();
        fs::create_dir_all(dir.join("generated")).unwrap();
        fs::write(dir.join("app/x/index.ts"), "").unwrap();
        fs::write(dir.join("generated/x.mts"), "").unwrap();
        let roots = [dir.join("app"), dir.join("generated")];
        assert_eq!(
            resolve_specifier(&dir.join("app"), "./x", &roots),
            Some("./x/index.js".to_string())
        );
    }

    // With overlapping roots only the longest root containing the source maps to the others:
    // a source under src/gen must not be treated as living at src's "gen" subdirectory.
    #[test]
    fn resolve_specifier_uses_longest_matching_root() {
        let dir = test_dir("resolve_root_dirs_longest");
        fs::create_dir_all(dir.join("src/gen")).unwrap();
        fs::create_dir_all(dir.join("other/gen")).unwrap();
        fs::write(dir.join("src/x.ts"), "").unwrap();
        fs::write(dir.join("other/gen/x.tsx"), "").unwrap();
        let roots = [dir.join("src"), dir.join("src/gen"), dir.join("other")];
        assert_eq!(
            resolve_specifier(&dir.join("src/gen"), "./x", &roots),
            Some("./x.js".to_string())
        );
    }

    // Roots are matched against the normalized import target, so a "../" specifier crossing out
    // of an overlapping root still maps into the other roots.
    #[test]
    fn resolve_specifier_parent_import_across_roots() {
        let dir = test_dir("resolve_root_dirs_parent");
        fs::create_dir_all(dir.join("src/gen")).unwrap();
        fs::create_dir_all(dir.join("other")).unwrap();
        fs::write(dir.join("other/x.tsx"), "").unwrap();
        let roots = [dir.join("src"), dir.join("src/gen"), dir.join("other")];
        assert_eq!(
            resolve_specifier(&dir.join("src/gen"), "../x", &roots),
            Some("../x.js".to_string())
        );
    }

    // A directory whose index exists only as a declaration file resolves like tsc, assuming the
    // JS implementation exists at runtime.
    #[test]
    fn resolve_specifier_resolves_declaration_index() {
        let dir = test_dir("resolve_dts_index");
        fs::create_dir_all(dir.join("gen")).unwrap();
        fs::create_dir_all(dir.join("genm")).unwrap();
        fs::write(dir.join("gen/index.d.ts"), "").unwrap();
        fs::write(dir.join("genm/index.d.mts"), "").unwrap();
        assert_eq!(
            resolve_specifier(&dir, "./gen", &[]),
            Some("./gen/index.js".to_string())
        );
        assert_eq!(
            resolve_specifier(&dir, "./genm", &[]),
            Some("./genm/index.mjs".to_string())
        );
    }

    #[test]
    fn resolve_specifier_outside_roots_unaffected() {
        let dir = test_dir("resolve_root_dirs_outside");
        fs::create_dir_all(dir.join("app")).unwrap();
        fs::create_dir_all(dir.join("other")).unwrap();
        fs::write(dir.join("app/y.ts"), "").unwrap();
        let roots = [dir.join("app"), dir.join("lib")];
        assert_eq!(resolve_specifier(&dir.join("other"), "./y", &roots), None);
    }

    #[test]
    fn resolve_specifier_resolves_sibling_declaration() {
        let dir = test_dir("resolve_sibling_dts");
        fs::write(dir.join("gen.d.ts"), "").unwrap();
        fs::write(dir.join("genc.d.cts"), "").unwrap();
        assert_eq!(resolve_specifier(&dir, "./gen", &[]), Some("./gen.js".to_string()));
        assert_eq!(
            resolve_specifier(&dir, "./genc", &[]),
            Some("./genc.cjs".to_string())
        );
    }
}
