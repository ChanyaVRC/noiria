//! `--autotest-script <file>` の行指向 DSL（パーサ + 実行状態機械）。
//!
//! ```text
//! # コメント（行頭・行末）
//! wait 500                       仮想時間で 500ms 待つ
//! click 1150 600                 左クリック（直前に同座標の move を送る）
//! rclick 640 360                 右クリック
//! move 160 560                   マウス移動
//! key Enter / keyup Control      キー押下 / 離し（Enter Escape Control Shift Space ArrowUp … または 1 文字）
//! shot title                     現在フレームを <out>/title.png へ保存
//! repeat 20 { click 1150 600 ; wait 400 }    1 行完結の繰り返し（入れ子可）
//! ```
//!
//! コマンドは `;` で区切れる（アリティが固定なので省略も可）。`repeat` は構文上その場で
//! 展開し、実行側は平らな [`Step`] 列だけを見る。時間は仮想クロック（`headless.rs`）基準。

use anyhow::{bail, Result};

/// 展開後ステップ数の上限（`repeat 999999999 {…}` のような誤記でメモリを食い潰さないため）。
const MAX_STEPS: usize = 2_000_000;

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Wait(u64),
    Move(f64, f64),
    Click(f64, f64, Button),
    Key(String),
    KeyUp(String),
    Shot(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

fn tokenize(line: &str) -> Vec<String> {
    let line = line.split('#').next().unwrap_or("");
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in line.chars() {
        match c {
            '{' | '}' | ';' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                out.push(c.to_string());
            }
            c if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

struct Cursor {
    toks: Vec<String>,
    pos: usize,
}

impl Cursor {
    fn peek(&self) -> Option<&str> {
        self.toks.get(self.pos).map(String::as_str)
    }

    fn next(&mut self) -> Option<String> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn arg(&mut self, cmd: &str, what: &str) -> Result<String> {
        match self.next() {
            Some(t) if !matches!(t.as_str(), "{" | "}" | ";") => Ok(t),
            _ => bail!("`{cmd}` の引数 <{what}> がありません"),
        }
    }

    fn num(&mut self, cmd: &str, what: &str) -> Result<f64> {
        let t = self.arg(cmd, what)?;
        t.parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .ok_or_else(|| anyhow::anyhow!("`{cmd}` の <{what}> が数値ではありません: {t}"))
    }
}

fn key_name(raw: &str) -> String {
    if raw == "Space" {
        " ".to_string()
    } else {
        raw.to_string()
    }
}

fn parse_cmds(c: &mut Cursor, nested: bool, out: &mut Vec<Step>) -> Result<()> {
    loop {
        match c.peek() {
            None => {
                if nested {
                    bail!("`repeat {{` が `}}` で閉じられていません");
                }
                return Ok(());
            }
            Some(";") => {
                c.pos += 1;
            }
            Some("}") => {
                if !nested {
                    bail!("対応する `repeat {{` の無い `}}` があります");
                }
                return Ok(());
            }
            Some(_) => {
                let cmd = c.next().unwrap();
                match cmd.as_str() {
                    "wait" => {
                        let ms = c.num("wait", "ms")?;
                        if ms < 0.0 {
                            bail!("`wait` の <ms> は 0 以上にしてください: {ms}");
                        }
                        out.push(Step::Wait(ms as u64));
                    }
                    "click" | "rclick" => {
                        let (x, y) = (c.num(&cmd, "x")?, c.num(&cmd, "y")?);
                        let b = if cmd == "click" { Button::Left } else { Button::Right };
                        out.push(Step::Click(x, y, b));
                    }
                    "move" => {
                        let (x, y) = (c.num("move", "x")?, c.num("move", "y")?);
                        out.push(Step::Move(x, y));
                    }
                    "key" => out.push(Step::Key(key_name(&c.arg("key", "name")?))),
                    "keyup" => out.push(Step::KeyUp(key_name(&c.arg("keyup", "name")?))),
                    "shot" => {
                        let name = c.arg("shot", "name")?;
                        if !name
                            .chars()
                            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
                        {
                            bail!("`shot` の名前は英数字・`_`・`-` のみ使えます: {name}");
                        }
                        out.push(Step::Shot(name));
                    }
                    "repeat" => {
                        let n = c.num("repeat", "n")?;
                        if n < 0.0 || n.fract() != 0.0 {
                            bail!("`repeat` の <n> は 0 以上の整数にしてください: {n}");
                        }
                        if c.next().as_deref() != Some("{") {
                            bail!("`repeat <n>` の後に `{{` が必要です");
                        }
                        let mut body = Vec::new();
                        parse_cmds(c, true, &mut body)?;
                        c.pos += 1; // 閉じ `}`
                        let n = n as usize;
                        if body.len().saturating_mul(n).saturating_add(out.len()) > MAX_STEPS {
                            bail!("`repeat` の展開後ステップ数が上限 {MAX_STEPS} を超えます");
                        }
                        for _ in 0..n {
                            out.extend(body.iter().cloned());
                        }
                    }
                    other => bail!("不明なコマンド `{other}`"),
                }
            }
        }
    }
}

/// DSL を展開済みステップ列へ変換する。エラーは `行番号: 内容` 形式。
pub fn parse(text: &str) -> Result<Vec<Step>> {
    let mut steps = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let toks = tokenize(line);
        if toks.is_empty() {
            continue;
        }
        let mut c = Cursor { toks, pos: 0 };
        parse_cmds(&mut c, false, &mut steps)
            .map_err(|e| anyhow::anyhow!("{}行目: {e}", i + 1))?;
        if steps.len() > MAX_STEPS {
            bail!("{}行目: ステップ数が上限 {MAX_STEPS} を超えます", i + 1);
        }
    }
    Ok(steps)
}

/// 仮想時間駆動の実行状態機械。`poll` は「いま実行すべき次のステップ」を 1 つずつ返し、
/// `Wait` に当たると以降の `poll` は期限まで `None` を返す。副作用（イベント送信・撮影）は
/// 呼び出し側（`headless.rs`）が行う。
pub struct Runner {
    steps: Vec<Step>,
    pc: usize,
    resume_at_ms: u64,
}

impl Runner {
    pub fn new(steps: Vec<Step>) -> Self {
        Self {
            steps,
            pc: 0,
            resume_at_ms: 0,
        }
    }

    pub fn finished(&self, now_ms: u64) -> bool {
        self.pc >= self.steps.len() && now_ms >= self.resume_at_ms
    }

    pub fn poll(&mut self, now_ms: u64) -> Option<Step> {
        if now_ms < self.resume_at_ms {
            return None;
        }
        while let Some(step) = self.steps.get(self.pc).cloned() {
            self.pc += 1;
            match step {
                Step::Wait(ms) => {
                    self.resume_at_ms = now_ms + ms;
                    if ms > 0 {
                        return None;
                    }
                }
                other => return Some(other),
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(text: &str) -> String {
        format!("{:#}", parse(text).unwrap_err())
    }

    #[test]
    fn parses_all_commands_and_comments() {
        let s = parse(
            "# 先頭コメント\n\
             wait 500   # 行末コメント\n\
             \n\
             click 1150 600\n\
             rclick 10.5 20\n\
             move 160 560\n\
             key Enter\n\
             key Space\n\
             keyup Control\n\
             shot title_01\n",
        )
        .unwrap();
        assert_eq!(
            s,
            vec![
                Step::Wait(500),
                Step::Click(1150.0, 600.0, Button::Left),
                Step::Click(10.5, 20.0, Button::Right),
                Step::Move(160.0, 560.0),
                Step::Key("Enter".into()),
                Step::Key(" ".into()),
                Step::KeyUp("Control".into()),
                Step::Shot("title_01".into()),
            ]
        );
    }

    #[test]
    fn repeat_expands_inline_and_nests() {
        let s = parse("repeat 3 { click 1 2 ; wait 10 }").unwrap();
        assert_eq!(s.len(), 6);
        assert_eq!(s[0], Step::Click(1.0, 2.0, Button::Left));
        assert_eq!(s[1], Step::Wait(10));
        assert_eq!(s[4], Step::Click(1.0, 2.0, Button::Left));

        let n = parse("repeat 2 { repeat 3 {key a} ; shot x }").unwrap();
        assert_eq!(n.len(), 2 * (3 + 1));
        // `;` は省略可、波括弧は隣接していてもよい
        let a = parse("repeat 2{wait 5 key a}").unwrap();
        assert_eq!(a.len(), 4);
        assert_eq!(parse("repeat 0 { key a }").unwrap(), vec![]);
    }

    #[test]
    fn multiple_commands_on_one_line_without_repeat() {
        let s = parse("move 1 2 ; click 3 4 ; wait 7").unwrap();
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn errors_report_line_numbers() {
        assert!(err("wait 1\nfoo 1").starts_with("2行目"));
        assert!(err("wait 1\nclick 1").contains("<y>"));
        assert!(err("click 1 abc").contains("数値"));
        assert!(err("# c\n\nrepeat 2 { key a").contains("閉じられていません"));
        assert!(err("key a }").contains("対応する"));
        assert!(err("repeat x { }").contains("数値"));
        assert!(err("repeat 1.5 { key a }").contains("整数"));
        assert!(err("repeat 2 key a").contains("`{`"));
        assert!(err("shot a/b").contains("英数字"));
        assert!(err("wait -5").contains("0 以上"));
        assert!(err("key").contains("<name>"));
    }

    #[test]
    fn expansion_is_capped() {
        assert!(err("repeat 3000 { repeat 3000 { key a } }").contains("上限"));
    }

    #[test]
    fn runner_honours_virtual_time_waits() {
        let steps = parse("key a\nwait 100\nkey b\nkey c\nwait 0\nkey d").unwrap();
        let mut r = Runner::new(steps);
        assert_eq!(r.poll(0), Some(Step::Key("a".into())));
        assert_eq!(r.poll(0), None, "Wait に入る");
        assert_eq!(r.poll(99), None);
        assert!(!r.finished(99));
        assert_eq!(r.poll(100), Some(Step::Key("b".into())));
        assert_eq!(r.poll(100), Some(Step::Key("c".into())));
        assert_eq!(r.poll(100), Some(Step::Key("d".into())), "wait 0 は待たない");
        assert_eq!(r.poll(100), None);
        assert!(r.finished(100));
    }

    #[test]
    fn runner_wait_is_relative_to_when_it_starts() {
        let mut r = Runner::new(parse("wait 50\nkey a\nwait 50\nkey b").unwrap());
        assert_eq!(r.poll(1000), None);
        assert_eq!(r.poll(1049), None);
        assert_eq!(r.poll(1050), Some(Step::Key("a".into())));
        assert_eq!(r.poll(1050), None);
        assert_eq!(r.poll(1100), Some(Step::Key("b".into())));
        assert!(r.finished(1100));
    }
}
