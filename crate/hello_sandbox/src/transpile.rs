use deno_ast::{
    parse_module, EmitOptions, MediaType, ParseParams, SourceMapOption, TranspileModuleOptions,
    TranspileOptions,
};

use crate::SandboxError;

/// Cheap heuristic: returns `true` if `source` contains TypeScript-only syntax.
/// Used to skip the parse cost for plain JavaScript files.
pub fn looks_like_typescript(source: &str) -> bool {
    source.contains(": string")
        || source.contains(": number")
        || source.contains(": boolean")
        || source.contains("interface ")
        || source.contains("enum ")
        || source.contains(": {")
        || source.contains("<T>")
        || source.contains(" as ")
        || source.contains("readonly ")
        || source.contains(": Promise<")
}

/// Transpile TypeScript (or TSX) source to ES2022 JavaScript.
///
/// Returns the original string unchanged when the input is plain JavaScript.
///
/// # Arguments
/// * `specifier` — module URL used for error messages and source map attribution.
/// * `source`    — raw source text.
/// * `force_ts`  — when `true`, always treat `source` as TypeScript even if the
///   heuristic or specifier extension says otherwise.
pub fn transpile(specifier: &str, source: &str, force_ts: bool) -> Result<String, SandboxError> {
    let media_type = if force_ts || specifier.ends_with(".ts") || specifier.ends_with(".tsx") {
        if specifier.ends_with(".tsx") {
            MediaType::Tsx
        } else {
            MediaType::TypeScript
        }
    } else if looks_like_typescript(source) {
        MediaType::TypeScript
    } else {
        // Plain JS — return unchanged.
        return Ok(source.to_string());
    };

    let parsed = parse_module(ParseParams {
        specifier: deno_ast::ModuleSpecifier::parse(specifier)
            .map_err(|e| SandboxError::TranspileError(e.to_string()))?,
        text: source.into(),
        media_type,
        capture_tokens: false,
        scope_analysis: false,
        maybe_syntax: None,
    })
    .map_err(|e| SandboxError::TranspileError(e.to_string()))?;

    let transpiled = parsed
        .transpile(
            &TranspileOptions::default(),
            &TranspileModuleOptions::default(),
            &EmitOptions {
                source_map: SourceMapOption::None,
                ..Default::default()
            },
        )
        .map_err(|e| SandboxError::TranspileError(e.to_string()))?;

    Ok(transpiled.into_source().text)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_js_passes_through_unchanged() {
        let src = "const x = 1 + 2; export default x;";
        let result = transpile("file:///test.js", src, false).unwrap();
        assert_eq!(result, src);
    }

    #[test]
    fn typescript_type_annotation_transpiles() {
        let src = r#"const x: number = 42; export default x;"#;
        let result = transpile("file:///test.ts", src, false).unwrap();
        // Type annotation stripped — colon-type form gone
        assert!(!result.contains(": number"), "type annotation should be erased");
        assert!(result.contains("42"), "value should be preserved");
    }

    #[test]
    fn interface_stripped() {
        let src = "interface Foo { bar: string; }\nconst f: Foo = { bar: 'x' };";
        let result = transpile("file:///test.ts", src, false).unwrap();
        assert!(!result.contains("interface"), "interface should be erased");
    }

    #[test]
    fn invalid_typescript_returns_transpile_error() {
        let src = "const x: = 1;"; // syntax error
        let err = transpile("file:///bad.ts", src, true).unwrap_err();
        match err {
            SandboxError::TranspileError(_) => {},
            other => panic!("expected TranspileError, got: {other:?}"),
        }
    }

    #[test]
    fn tsx_transpiles() {
        let src = r#"
            const el: JSX.Element = <div className="foo">Hello</div>;
            export default el;
        "#;
        let result = transpile("file:///comp.tsx", src, false).unwrap();
        assert!(!result.contains("JSX.Element"), "TSX type should be erased");
    }

    #[test]
    fn force_ts_bypasses_heuristic() {
        // Source has no obvious TS tokens, but force_ts=true
        let src = "const x = 42; export default x;";
        // Should still succeed (it's valid TS/JS)
        let result = transpile("file:///test.js", src, true).unwrap();
        assert!(result.contains("42"));
    }

    #[test]
    fn looks_like_typescript_detects_annotations() {
        assert!(looks_like_typescript("const x: string = '';"));
        assert!(looks_like_typescript("const x: number = 0;"));
        assert!(looks_like_typescript("interface Foo {}"));
        assert!(looks_like_typescript("enum Color { Red }"));
        assert!(looks_like_typescript("const x = val as string;"));
        assert!(!looks_like_typescript("const x = 1 + 2;"));
        assert!(!looks_like_typescript("function foo(a, b) { return a + b; }"));
    }
}
