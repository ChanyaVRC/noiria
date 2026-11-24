// noiria:kag/parser — KAG3 風タグシナリオ (.ks) のパーサ。
// 純関数なのでヘッドレスでテストできる。
//
// 行の種類:
//   ; コメント
//   *ラベル名|表示名
//   @タグ 属性=値 ...          (行タグ)
//   本文 [タグ 属性=値] 本文 ...  (インラインタグ混在)
//   [[ は literal '[' のエスケープ

export type Attrs = Record<string, string>;

export type Command =
  | { type: "label"; name: string; display: string | null; line: number }
  | { type: "tag"; name: string; attrs: Attrs; line: number }
  | { type: "text"; text: string; line: number }
  /** [iscript]〜[endscript] 間の生テキストを 1 コマンドとして収集したもの（W6）。
   * 中身は KAG タグとして解釈せず、実行時に vars.evalStmt へそのまま渡す（TS として評価）。 */
  | { type: "iscript"; code: string; line: number };

export interface Scenario {
  file: string;
  commands: Command[];
  labels: Map<string, number>;
}

/** [iscript]/[endscript] を単独行として認識する（属性は受理しない、素の開始/終了タグのみ）。 */
const ISCRIPT_OPEN_RE = /^\[iscript\s*\]$/;
const ISCRIPT_CLOSE_RE = /^\[endscript\s*\]$/;

/**
 * lines[startIdx] 以降を、末尾が [endscript]（または @endscript）の行に一致するまで
 * 生テキストのまま連結する（KAG タグとして解釈しない。`[`/`]` を含む TS コードを
 * 誤ってタグ扱いしないための専用モード）。閉じタグが見つからない場合はファイル終端まで
 * 本文として扱う（`docs/implementation-notes.md`「2026-07-11 W6」参照）。
 */
function collectIscriptBody(lines: string[], startIdx: number): { code: string; endIdx: number } {
  const body: string[] = [];
  let i = startIdx;
  for (; i < lines.length; i++) {
    const trimmed = lines[i].trim();
    if (ISCRIPT_CLOSE_RE.test(trimmed) || trimmed === "@endscript") {
      return { code: body.join("\n"), endIdx: i };
    }
    body.push(lines[i]);
  }
  return { code: body.join("\n"), endIdx: lines.length - 1 };
}

export function parseScenario(source: string, file: string): Scenario {
  const commands: Command[] = [];
  const labels = new Map<string, number>();
  const lines = source.split(/\r\n|\n|\r/);

  for (let i = 0; i < lines.length; i++) {
    const lineNo = i + 1;
    const raw = lines[i];
    const line = raw.trim();
    if (line.length === 0) continue;
    if (line.startsWith(";")) continue;

    if (line.startsWith("*")) {
      const body = line.slice(1);
      const sep = body.indexOf("|");
      const name = (sep >= 0 ? body.slice(0, sep) : body).trim();
      const display = sep >= 0 ? body.slice(sep + 1).trim() : null;
      labels.set(name, commands.length);
      commands.push({ type: "label", name, display, line: lineNo });
      continue;
    }

    if (line.startsWith("@")) {
      const { name, attrs } = parseTagBody(line.slice(1));
      if (name === "iscript") {
        const { code, endIdx } = collectIscriptBody(lines, i + 1);
        commands.push({ type: "iscript", code, line: lineNo });
        i = endIdx;
        continue;
      }
      commands.push({ type: "tag", name, attrs, line: lineNo });
      continue;
    }

    if (ISCRIPT_OPEN_RE.test(line)) {
      const { code, endIdx } = collectIscriptBody(lines, i + 1);
      commands.push({ type: "iscript", code, line: lineNo });
      i = endIdx;
      continue;
    }

    // インライン行: テキストとタグの混在
    parseInline(line, lineNo, commands);
  }

  return { file, commands, labels };
}

function isWsCode(c: number): boolean {
  return (
    c === 32 ||
    c === 9 ||
    c === 10 ||
    c === 13 ||
    c === 12 ||
    c === 11 ||
    c === 0x3000 ||
    c === 0xa0 ||
    c === 0xfeff
  );
}

function parseInline(line: string, lineNo: number, out: Command[]): void {
  const len = line.length;
  let text = "";
  let textStart = 0;
  let i = 0;
  const flush = (end: number) => {
    if (end > textStart) {
      text += line.slice(textStart, end);
    }
    if (text.length > 0) {
      out.push({ type: "text", text, line: lineNo });
      text = "";
    }
  };
  while (i < len) {
    if (line.charCodeAt(i) === 91 /* '[' */) {
      if (i + 1 < len && line.charCodeAt(i + 1) === 91 /* '[' */) {
        text += line.slice(textStart, i + 1);
        i += 2;
        textStart = i;
        continue;
      }
      // 閉じ ] を探す（クォート内は無視）
      let j = i + 1;
      let quote = 0;
      while (j < len) {
        const cj = line.charCodeAt(j);
        if (quote !== 0) {
          if (cj === quote) quote = 0;
        } else if (cj === 34 /* '"' */ || cj === 39 /* '\'' */) {
          quote = cj;
        } else if (cj === 93 /* ']' */) {
          break;
        }
        j++;
      }
      if (j >= len) {
        // 閉じられていない [ は本文扱い
        flush(len);
        return;
      }
      flush(i);
      const { name, attrs } = parseTagBody(line.slice(i + 1, j));
      out.push({ type: "tag", name, attrs, line: lineNo });
      i = j + 1;
      textStart = i;
      continue;
    }
    i++;
  }
  flush(len);
}

function parseTagBody(body: string): { name: string; attrs: Attrs } {
  const attrs: Attrs = {};
  const len = body.length;
  let i = 0;
  const skipWs = () => {
    while (i < len && isWsCode(body.charCodeAt(i))) i++;
  };
  skipWs();
  const nameStart = i;
  while (i < len && !isWsCode(body.charCodeAt(i))) i++;
  const name = body.slice(nameStart, i);
  skipWs();
  while (i < len) {
    const keyStart = i;
    while (i < len) {
      const c = body.charCodeAt(i);
      if (c === 61 /* '=' */ || isWsCode(c)) break;
      i++;
    }
    const key = body.slice(keyStart, i);
    skipWs();
    if (i < len && body.charCodeAt(i) === 61 /* '=' */) {
      i++;
      skipWs();
      let value = "";
      if (i < len) {
        const q = body.charCodeAt(i);
        if (q === 34 /* '"' */ || q === 39 /* '\'' */) {
          i++;
          const valStart = i;
          while (i < len && body.charCodeAt(i) !== q) i++;
          value = body.slice(valStart, i);
          if (i < len) i++; // 閉じクォート
        } else {
          const valStart = i;
          while (i < len && !isWsCode(body.charCodeAt(i))) i++;
          value = body.slice(valStart, i);
        }
      }
      if (key) attrs[key] = value;
    } else if (key) {
      // 値なし属性は true 扱い
      attrs[key] = "true";
    }
    skipWs();
  }
  return { name: name.toLowerCase(), attrs };
}
