//! エンジン組み込みの TS モジュール一覧。V8 (`loader.rs`) / tsint (`tsint_engine.rs`)
//! の両方から共用する（W7: どちらか一方の engine feature だけを有効にしたビルドでも
//! このテーブル自体は常にコンパイルされる必要があるため、`loader.rs`（engine-v8 限定）
//! から切り出した）。

pub(crate) static RUNTIME_MODULES: &[(&str, &str)] = &[
    ("noiria:api", include_str!("../../../../runtime/api.ts")),
    ("noiria:kag", include_str!("../../../../runtime/kag/mod.ts")),
    (
        "noiria:kag/anim",
        include_str!("../../../../runtime/kag/anim.ts"),
    ),
    (
        "noiria:kag/buttons",
        include_str!("../../../../runtime/kag/buttons.ts"),
    ),
    (
        "noiria:kag/menu",
        include_str!("../../../../runtime/kag/menu.ts"),
    ),
    (
        "noiria:kag/mhud",
        include_str!("../../../../runtime/kag/mhud.ts"),
    ),
    (
        "noiria:kag/mmenu",
        include_str!("../../../../runtime/kag/mmenu.ts"),
    ),
    (
        "noiria:kag/mtitle",
        include_str!("../../../../runtime/kag/mtitle.ts"),
    ),
    (
        "noiria:kag/mui",
        include_str!("../../../../runtime/kag/mui.ts"),
    ),
    (
        "noiria:kag/parser",
        include_str!("../../../../runtime/kag/parser.ts"),
    ),
    (
        "noiria:kag/player",
        include_str!("../../../../runtime/kag/player.ts"),
    ),
    (
        "noiria:kag/tags",
        include_str!("../../../../runtime/kag/tags.ts"),
    ),
    (
        "noiria:kag/ui",
        include_str!("../../../../runtime/kag/ui.ts"),
    ),
    (
        "noiria:kag/vars",
        include_str!("../../../../runtime/kag/vars.ts"),
    ),
];
