//! ops 共通の定型処理。

use deno_error::JsErrorBox;

pub fn err(e: impl std::fmt::Display) -> JsErrorBox {
    JsErrorBox::generic(e.to_string())
}
