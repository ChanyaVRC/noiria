//! モジュールローダ。
//! - `noiria:` プレフィクス → include_str! で埋め込んだエンジン TS ライブラリ
//! - `file://` (ゲームルート配下のみ) → ディスクから読む
//!
//! どちらも TypeScript は deno_ast (swc) でその場で JS に変換する。

use deno_ast::{
    EmitOptions, MediaType, ParseParams, SourceMapOption, TranspileModuleOptions, TranspileOptions,
};
use deno_core::{
    resolve_import, ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleLoader,
    ModuleResolveResponse, ModuleSource, ModuleSourceCode, ModuleSpecifier, ModuleType,
    RequestedModuleType, ResolutionKind,
};
use deno_error::JsErrorBox;
use noiria_core::assets::GameDir;

use crate::script::runtime_modules::RUNTIME_MODULES;

pub(crate) struct NoiriaModuleLoader {
    /// W5: 実読込は `GameDir` に一本化（`file://` はゲームルート相対 `rel` へ変換
    /// してから `GameDir::read_text` を呼ぶ。tsint 側 `NoiriaTsHost` と同構図）。
    game_dir: GameDir,
    /// ゲームルートの file:// URL（末尾スラッシュ付き）。モジュール解決
    /// （相対 import の URL join）にのみ使う。
    game_root: ModuleSpecifier,
    /// `game_root.to_file_path()` の結果（`load()` 側で読むモジュールの絶対パスと
    /// 同じ変換経路を通した値）。Windows では `canonicalize()`（`GameDir::root()`）が
    /// `\\?\` verbatim prefix 付きのパスを返す一方、`Url::to_file_path()` は
    /// verbatim prefix を付けないため、`GameDir::root()` を直接 `strip_prefix` すると
    /// 常に不一致になってしまう。両者を同じ `to_file_path()` 経由の値で揃えることで
    /// この不一致を避ける。
    game_root_path: std::path::PathBuf,
}

impl NoiriaModuleLoader {
    pub(crate) fn new(game_dir: GameDir) -> anyhow::Result<Self> {
        let mut url = ModuleSpecifier::from_directory_path(game_dir.root())
            .map_err(|_| anyhow::anyhow!("ゲームルートを URL に変換できません"))?;
        if !url.path().ends_with('/') {
            url.set_path(&format!("{}/", url.path()));
        }
        let game_root_path = url
            .to_file_path()
            .map_err(|_| anyhow::anyhow!("ゲームルート URL をパスに変換できません"))?;
        Ok(Self {
            game_dir,
            game_root: url,
            game_root_path,
        })
    }

    pub(crate) fn main_module(&self, rel: &str) -> Result<ModuleSpecifier, JsErrorBox> {
        self.game_root
            .join(rel)
            .map_err(|e| JsErrorBox::generic(e.to_string()))
    }
}

/// TypeScript / JSX を JavaScript へ変換する。JS はそのまま返す。
fn maybe_transpile(
    specifier: &ModuleSpecifier,
    media_type: MediaType,
    code: String,
) -> Result<String, JsErrorBox> {
    let needs = matches!(
        media_type,
        MediaType::TypeScript
            | MediaType::Mts
            | MediaType::Cts
            | MediaType::Tsx
            | MediaType::Jsx
            | MediaType::Dts
    );
    if !needs {
        return Ok(code);
    }
    let parsed = deno_ast::parse_module(ParseParams {
        specifier: specifier.clone(),
        text: code.into(),
        media_type,
        capture_tokens: false,
        scope_analysis: false,
        maybe_syntax: None,
    })
    .map_err(|e| JsErrorBox::generic(format!("TS パースエラー {specifier}: {e}")))?;
    let emitted = parsed
        .transpile(
            &TranspileOptions::default(),
            &TranspileModuleOptions::default(),
            &EmitOptions {
                source_map: SourceMapOption::Inline,
                ..Default::default()
            },
        )
        .map_err(|e| JsErrorBox::generic(format!("TS 変換エラー {specifier}: {e}")))?
        .into_source();
    Ok(emitted.text)
}

impl ModuleLoader for NoiriaModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> ModuleResolveResponse {
        if specifier.starts_with("noiria:") {
            return ModuleSpecifier::parse(specifier)
                .map_err(|e| JsErrorBox::generic(e.to_string()));
        }
        resolve_import(specifier, referrer).map_err(|e| JsErrorBox::generic(e.to_string()))
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&ModuleLoadReferrer>,
        options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        let specifier = module_specifier.clone();
        let result = (|| -> Result<ModuleSource, JsErrorBox> {
            let (code, media_type) = match specifier.scheme() {
                "noiria" => {
                    let key = specifier.as_str();
                    let src = RUNTIME_MODULES
                        .iter()
                        .find(|(k, _)| *k == key)
                        .map(|(_, v)| *v)
                        .ok_or_else(|| {
                            JsErrorBox::generic(format!("組み込みモジュールがありません: {key}"))
                        })?;
                    (src.to_string(), MediaType::TypeScript)
                }
                "file" => {
                    // ゲームルート相対 `rel` へ変換してから `GameDir::read_text` 経由で読む
                    // （W5: パスジェイルも GameDir 側の resolve() に一本化）。
                    let path = specifier.to_file_path().map_err(|_| {
                        JsErrorBox::generic(format!("パスに変換できません: {specifier}"))
                    })?;
                    let rel = path.strip_prefix(&self.game_root_path).map_err(|_| {
                        JsErrorBox::generic(format!(
                            "ゲームディレクトリ外のモジュールは読み込めません: {specifier}"
                        ))
                    })?;
                    let code = self
                        .game_dir
                        .read_text(&rel.to_string_lossy())
                        .map_err(|e| {
                            JsErrorBox::generic(format!(
                                "モジュールが読めません {}: {e:#}",
                                path.display()
                            ))
                        })?;
                    (code, MediaType::from_specifier(&specifier))
                }
                other => return Err(JsErrorBox::generic(format!("未対応のスキーム: {other}"))),
            };

            if matches!(options.requested_module_type, RequestedModuleType::Json) {
                return Ok(ModuleSource::new(
                    ModuleType::Json,
                    ModuleSourceCode::String(code.into()),
                    &specifier,
                    None,
                ));
            }

            let js = maybe_transpile(&specifier, media_type, code)?;
            Ok(ModuleSource::new(
                ModuleType::JavaScript,
                ModuleSourceCode::String(js.into()),
                &specifier,
                None,
            ))
        })();
        ModuleLoadResponse::Sync(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transpile_strips_types() {
        let spec = ModuleSpecifier::parse("noiria:test").unwrap();
        let ts = r#"
export interface Foo { a: number; }
export function add(a: number, b: number): number { return a + b; }
const x: Foo = { a: add(1, 2) };
export default x;
"#;
        let js = maybe_transpile(&spec, MediaType::TypeScript, ts.to_string()).unwrap();
        assert!(!js.contains("interface"));
        assert!(!js.contains(": number"));
        assert!(js.contains("function add"));
    }

    #[test]
    fn plain_js_passthrough() {
        let spec = ModuleSpecifier::parse("file:///a.js").unwrap();
        let src = "export const a = 1;".to_string();
        let js = maybe_transpile(&spec, MediaType::JavaScript, src.clone()).unwrap();
        assert_eq!(js, src);
    }

    #[test]
    fn runtime_modules_all_transpile() {
        // 埋め込み TS ライブラリ全部が構文エラーなく JS になることを確認
        for (name, src) in RUNTIME_MODULES {
            let spec = ModuleSpecifier::parse(name).unwrap();
            let result = maybe_transpile(&spec, MediaType::TypeScript, src.to_string());
            assert!(result.is_ok(), "{name} の変換に失敗: {:?}", result.err());
        }
    }
}
