# Proposal: hu-md — markdown に hubullu フォームを埋め込むツール

hubullu とは独立したツールで、散文ドキュメント（markdown）に hubullu のエントリー／フォームへの参照を埋め込み、それを実際の語形に解決する。**hubullu をライブラリとして利用するが、hubullu 本体には同梱しない。**

## 1. 動機

a priori 人工言語プロジェクト（`/Users/csakai/repos/writing/a_priori/run_01/`）は 2 つを並行管理している:

- `spec.md` — 設計仕様書（散文、日本語）
- `hu/*.hu` — hubullu 形式の転写（フォームの真の定義）

語形は**両方に現れる** — `spec.md` には手打ち、`hu/` には定義。両者は drift する。実際、設計セッション中に `bɯsɯbɯ` `mɯrəpap` `kɯtɯbəj` といったフォームが `spec.md` に未検証のまま手打ちされた。handoff ドキュメントもこの二重管理を既知の負債として明記している。

**目標**: 散文ドキュメントが hubullu のエントリー／フォームを参照し、ツールがそれを解決する。ドキュメント中のフォームは hubullu 定義から**生成・検証**され、手打ちされない。

**段階方針**: `<hu-token>` 等の埋め込み規約と `<!-- @reference -->` のプロジェクト宣言（§5）は **ツールが無くても今すぐ採用できる** — 生 HTML / HTML コメントは markdown を素通しするだけだから。手打ちフォームをやめて `.hut` 式を埋め込んだ時点で drift は構造的に消える（drift する「手打ち語形」が存在しなくなる）。解決・検証ツールは後から作ればよく、本 proposal はそのツールの設計記録である。ツールが無い間は `<hu-token>` の中身（.hut 式）がそのまま表示される、という表示上のトレードオフだけ存在する。

## 2. 設計（議論で確定した方針）

### 2.1 独立ツール・hubullu はライブラリ依存

- hubullu とは別の crate / バイナリ。hubullu 本体には同梱しない
- `hubullu` を**ライブラリ crate** として依存（`use hubullu;`）。解決エンジン（`.hu` コンパイル・`.hut` リゾルバ・`.huc` キャッシュ・SQLite emit）を再利用
- 理由: hubullu を辞書・文法コンパイラとして focused に保つ。markdown という別関心をコアに持ち込まない

### 2.2 埋め込みは生 HTML 要素

- markdown は**生 HTML を素通しする**。`<hu-token>` のようなカスタム要素はどの markdown プロセッサ（pandoc / remark / markdown-it / …）でも無改変で出力 HTML に到達する
- → ツールは **HTML → HTML のポストプロセッサ**でよい。markdown 処理の**後段に挟むだけ**で、特定の md エコシステムのプラグインにする必要がない
- → **新しい拡張子は要らない**。「`<hu-*>` 入りのただの `.md`」を通常の markdown パイプラインに通し、その出力 HTML を hu-md に通す

### 2.3 要素の中身 = `.hut` 断片

要素の中身に hubullu 既存の `.hut` トークン構文をそのまま使う。新しい構文表面はゼロ:

```html
<hu-token>kataba[question=decl, negation=aff, mood=ind, tense=past,
  aspect=simple, abs_number=sg, abs_person=3, erg_number=sg, erg_person=1]</hu-token>
```

ツールは中身を hubullu の `.hut` リゾルバに渡し、解決した表層形で要素を置換する。

## 3. 要素セット（案）

| 要素 | 用途 | 出力 |
|------|------|------|
| `<hu-token>` | 単一フォームの解決 | 表層形（インライン） |
| `<hu-gloss>` | 行間グロス | フォーム + 形態素分解 + 意味の表 |
| `<hu-paradigm>` | 屈折表 | 軸を展開した表 |
| `<hu-form>` | 語根 + テンプレートの素朴な適用 | 表層形 |

`<hu-token>` が最小の入口。**`spec.md` の本当の痛みはグロス例**（§3.8 や §4.3 の形態素分解テーブル）なので、`<hu-gloss>` が実際の価値の中心になる。

## 4. 解決バックエンド

hubullu のコンパイルは遅い（a priori proto で数分）。候補:

| 方式 | 速度 | 備考 |
|------|------|------|
| SQLite emit を query | 速い | hubullu が emit 済みのものに限られる |
| ライブラリ API で `.huc` に対し解決 | 速い・柔軟 | 事前コンパイル or インクリメンタルキャッシュ前提 |
| `hubullu render -e ...` をトークンごとに spawn | 遅い | プロセス起動オーバーヘッド |

**推奨**: 事前コンパイルした `.huc`（または hubullu のインクリメンタルキャッシュ）に対しライブラリ API で解決。コンパイルは一度、トークン解決は多数を高速に。

## 5. プロジェクト束縛

ドキュメントがどの hubullu プロジェクトに対して解決するかを **HTML コメント**で宣言する。`<hu-token>` と同じ理屈 — HTML コメントは markdown を素通しし、レンダー出力に現れず、hubullu 既存の `@reference` / `@use` directive 構文をそのまま再利用できる:

```
<!-- @reference * from "hu/main.hu" -->
```

ドキュメント先頭に置く。複数 directive 可。ツールが無くても書いておけるので後からいくらでも処理できる。CLI 引数での上書きは許容してもよい。

## 6. 失敗モード

解決できないトークン（typo・削除済みエントリー・不正なタグ）は **hard error でビルドを落とす**。素通しは厳禁 — **drift 検出こそがこのツールの存在意義**。

## 7. パイプライン統合

```
doc.md ──[markdown processor]──> doc.html ──[hu-md]──> doc.html (解決済み)
         pandoc / remark / ...              <hu-*> を実フォームに置換
```

hu-md は HTML を入力に取り HTML を出力する単純なフィルタ。CI に組み込めば、`spec.md` のフォームが `hu/` 定義と乖離した瞬間にビルドが落ちる。

## 8. 解決すべき設計判断

1. **要素セットの範囲** — `<hu-token>` のみで始めるか、最初から gloss/paradigm/form まで
2. **解決バックエンド** — SQLite query か ライブラリ API か
3. **出力形式** — HTML のみか、markdown に戻す経路（チェーン用）も持つか
4. **ツール名** — 仮称 `hu-md`
5. **gloss / paradigm の整形** — CSS クラス付き表か、プレーン表か。スタイルはドキュメント側に委ねるか
6. **インライン記法の糖衣** — `<hu-token>...</hu-token>` は冗長。`{{hu: kataba[...]}}` のような短縮を別途持つか（ただし markdown プロセッサ依存になるので素の HTML 要素を基本とする）

（プロジェクト束縛は §5 で `<!-- @reference -->` HTML コメントに確定。）

## 9. 実装フェーズ（案）

| Phase | 内容 |
|-------|------|
| 1 | HTML ポストプロセッサの骨格。HTML をパース → `<hu-token>` を発見 → hubullu lib で解決 → 置換。プロジェクト束縛は CLI 引数。失敗は hard error |
| 2 | `<hu-gloss>` — 行間グロスの描画。spec.md の例テーブルを賄う |
| 3 | `<hu-paradigm>` — 屈折表 |
| 4 | frontmatter 束縛、キャッシュ、パイプライン ergonomics |

## 10. hubullu 本体との関係

- hu-md は hubullu の**消費者**であり、hubullu に変更を要求しない（理想）
- ただし実装中に「ライブラリ API がフォーム単体解決に向いていない」等が判明したら、hubullu 側に小さな API 追加 proposal を立てる余地はある
- `.hut` フォーマットとの概念的重複（どちらも「文書 + 埋め込み参照」）は認識しているが、`.hut` は hubullu の HTML サイト生成用、hu-md は任意の markdown パイプライン用、と用途が分かれるため別ツールとして妥当

## Critical Files（実装時）

- hu-md は新規 crate。`hubullu` を path/crates 依存
- 参照する hubullu API: `.hu` コンパイル（`phase1`/`phase2`）、`.hut` リゾルバ（`render` 周辺）、`.huc` ロード、`emit_sqlite`（query する場合）
- HTML パースは crate に丸投げ（`scraper` / `html5ever` 等）
