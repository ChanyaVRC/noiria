// 星待ちの放課後 — Noiria デモ（恋愛アドベンチャー）のエントリポイント。

import { KagPlayer } from "noiria:kag";

const player = new KagPlayer({
  charaLayerCount: 2,
  autoWaitMs: 1800,
  // 市販 ADV 風の UI（下部の全幅の帯・中央寄せの本文カラム・選択肢ボタン・専用のタイトル/メニュー）。
  ui: "modern",
  // 帯の下端右寄りに Q.Save / Q.Load / Save / Load / Auto / Skip / Log / Config を常設する。
  systemButtons: true,
  // システムメニューの「タイトルへ戻る」の行き先。
  title: { storage: "title.ks", target: "*menu" },
  titleScreen: {
    name: "星待ちの放課後",
    subtitle: "— 流星群の夜まで、あと五日 —",
    tagline: "A STARGAZING ROMANCE ADVENTURE",
    version: "Ver 1.0.0　　© 2026 星待ち天文部",
    menu: [
      { label: "START", sub: "はじめから", target: "*start_game" },
      { label: "CONTINUE", sub: "つづきから", action: "continue" },
      { label: "GALLERY", sub: "おまけ", action: "gallery" },
      { label: "CONFIG", sub: "設定", action: "config" },
      { label: "EXIT", sub: "終了", action: "exit" },
    ],
  },
  // 到達したエンドの CG（sf のフラグで解放）。
  gallery: [
    { flag: "endHinata", image: "assets/cg/cg_hinata.png", title: "ひなたEND「いちばん近くの一等星」" },
    { flag: "endShiori", image: "assets/cg/cg_shiori.png", title: "しおりEND「来年の星も、あなたと」" },
    { flag: "endNormal", image: "assets/cg/cg_meteor.png", title: "ノーマルEND「三人の観測記録」" },
  ],
  sound: { click: "click.wav" },
});

await player.run("title.ks");
