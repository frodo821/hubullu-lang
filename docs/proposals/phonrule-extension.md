# Phonrule Extension Proposal

子孫言語（diachronic conlanging）への分岐ワークフローを支えるため、phonrule 周辺に5つの機能拡張を提案する。

別プロジェクト（a priori 祖語、`/Users/csakai/repos/writing/a_priori/run_01/`）で、proto 言語の `.hut` 例文を音韻変化チェーンで子孫言語に派生させたい、というユースケースが起点。

## 概要：5つの拡張

| ID | 機能 | 主目的 |
|----|------|--------|
| F1 | `.hut` の `@apply` directive ＋ インライン phonrule 関数（`~` = 膠着マーカー） | 例文に音韻変化を被せる |
| F2 | `phoneme` 宣言 ＋ `syllable` rule（音節分解と σ-aware phonrule） | 音節条件の音韻変化を記述 |
| F3 | `phonrule` body 内 `apply <other>`（合成） | stage 概念を別途立てずに済む |
| F4 | CLI `-e` フラグ（hubullu コード片の評価） | 実行時切替で同じ proto から複数子孫を render |
| F5 | セミコロン区切り（statement 境界限定） | CLI 渡しのワンライナー実現 |

---

## 1. 現状調査サマリー

**lexer**（`src/lexer.rs`、`src/token.rs`）— 改行は普通の whitespace としてスキップされ Newline トークンは生まれない。**セミコロンは現状全く認識されない**（`;` を入れると "unexpected character" になる）。`@`-directive は `@use / @reference / @extend / @export / @render` の5種が固定で hard-coded。未知の `@foo` はエラー（`lexer.rs:559`）。

**phonrule の現行構文**（`ast.rs:202–315`、`parser.rs:876`）— body は `class` / `map` / rewrite rule (`PATTERN -> REPLACEMENT / CONTEXT`) の3種のみ。**body 内に `apply` も他 phonrule の再利用構文も存在しない**。BOUNDARY は `\0` で morpheme 境界、`^`/`$` は word start/end、syllable 概念は皆無。`phonrule_eval.rs::apply_phonrule` は1つの PhonRule の rule 群を収束まで反復適用。

**phonrule の合成は inflection 側にしかない**：`apply harmony(elision(cell))` は `ApplyExpr` という別 AST（`ast.rs:365`）として inflection body の prefix にのみ書ける。phonrule の名前解決は `phase2.rs::find_phonrule` 経由でファイルスコープ。

**`.hut` parser**（`parser.rs:1490`）— 先頭で `@reference` 列を消化、その後 `parse_hut_tokens` でトークン列。トークン種は Ref / Lit / Glue(`~`) / Newline(`//`) / Tag。**`@apply`・インライン phonrule 関数・`@use` は全て未対応**。

**`~`（Glue）のセマンティクス**（`render.rs:1064 smart_join`）— *純粋にレンダリング層でセパレータを抑制するマーカー*。phonrule 適用やスペル変形は一切引き起こさない。各 Token は独立に resolve され、その後 join 時に Glue を見るだけ。

**CLI**（`src/main.rs`）— `clap` derive。`render` サブコマンドは `input` `--dir` `--outdir` `--huc` `--title` のみ。`-e` 系フラグなし。

**statement separator** — `.hu` も `.hut` も「文の終端」を表すトークンは存在せず、純粋に**構造マーカー（`{` `}` `[` `]` キーワード等）駆動でパース**。改行依存もセミコロン依存もない。なので `;` を導入しても既存の意味論にゼロ影響。

---

## 2. 機能仕様

### F1. `.hut` の `@apply` と インライン phonrule 関数

**`~` のセマンティクス（再解釈）**

現在の `~` は「レンダ層のセパレータ抑制」だが、これを **膠着マーカー (agglutination boundary)** として再解釈する。すなわち `~` で連結されたトークン列は「膠着的に貼り付いた1つの音韻語」を意味する。これは `.hu` 側 compose の `+`（形態素境界）の `.hut` 版に相当し、phonrule から見ても「1つの音韻語の内部境界」として自然に扱える。

セマンティクスの「変更」ではなく「本来そうだったものを phonrule 評価で活用するだけ」。既存ユーザーへの影響ゼロ。

**構文（最終形）**

```
hut_file        = (reference | apply_dir | use_dir)* token_seq ;
apply_dir       = "@apply" ident ;                  # file-scope
apply_block     = "@apply" ident "{" token_seq "}"; # nested scope
use_dir         = "@use" import_target "from" string_literal ;  # .hut でも @use 解禁
token           = ... | phon_call | apply_block ;
phon_call       = ident "(" token_seq ")" ;
```

**セマンティクス**

- **音韻語 (phonological word) = `~` で連結された Ref/Lit/PhonCall の極大連鎖**。スペース区切りトークンは独立した音韻語。
- file-level `@apply X`：その `.hut` の全音韻語に X を適用。複数 `@apply` は宣言順に左から右へチェイン適用。
- `@apply X { ... }`：ブロック内のみ X が追加。外側の active apply スタックの**末尾に push**してから評価、ブロック終端で pop。
- インライン `f(token_seq)`：token_seq を1つの音韻語に強制し、その文字列に f を適用。**外側 @apply の対象ではなく**「最内 phon_call」が優先（明示適用が暗黙適用を上書き）。これは inflection 側の `apply harmony(elision(cell))` セマンティクスと一致させる。
- 適用方式：各音韻語を resolve → 内部の Ref/Lit を `~` を BOUNDARY (`\0`) として連結 → active phonrule chain を順次 `apply_phonrule` → `strip_boundaries` → join。
- 既存 `~` のレンダ層セパレータ抑制も維持（膠着マーカーが「音韻語境界」と「render 上の連結」を兼ねる）。

**衝突なし**：phonrule を `@apply` していなければ動作不変（BOUNDARY 連結はするが、適用すべき rule が空なら no-op で素通り）。

**エッジケース判断**

- `@apply X { @apply Y { ... } }`：内側で X→Y の順に適用。
- ファイル先頭の `@apply` と `@reference` の順序：parser は両方を free-order で受ける（先頭ブロックでまとめて consume）。
- インライン `f(a ~ b ~ c)` 内部に `~` 不在 → a, b, c それぞれ独立に f 適用（既存トークン規約のとおり）。
- `@apply` した phonrule 名が見つからない → コンパイル時エラー（フェーズ2、`.hut` 用にも `find_phonrule` を流用、`@use` で取り込んだスコープに居る前提）。

### F2. phoneme 宣言 ＋ syllable rule

#### F2a. `phoneme` 宣言（音素集合の名前付け）

音素集合を **top-level の `phoneme` 宣言** として導入する。syllable rule・phonrule・複数ファイルから共有参照される、`@use`/`@export use` の対象。phonrule body 内 local の `class` とは併用（class は ad-hoc 補助、phoneme は global インベントリ）。

**構文**

```hu
phoneme vowels_front  { "a", "œ" }
phoneme vowels_back   { "ɯ", "o" }
phoneme vowels_neutral { "ə" }

phoneme V {
  vowels_front
  vowels_back
  vowels_neutral
}

phoneme stops_voiceless { "p", "t", "k" }
phoneme stops_voiced    { "b", "d", "g" }
phoneme nasals          { "m", "n", "ŋ" }
phoneme fricatives      { "s", "h" }
phoneme liquids         { "l", "r" }
phoneme glides          { "w", "j" }

phoneme C {
  stops_voiceless
  stops_voiced
  nasals
  fricatives
  liquids
  glides
}
```

- block 内には **リテラル文字列** と **他 phoneme 参照** を列挙、両者の union。
- **multigraph 対応**：phoneme 内のエントリーは longest-match-first で tokenize（"ng" と "n" + "g" のあいまいさは長い方が勝つ）。
- top-level 構文として hoisting 対象。
- 循環参照は phase2 で DFS 検出してエラー。
- phonrule body 内でも `phoneme` を直接参照可（既存 `class` の代わりに、または `class X = pheneme_ref | ...` の混合可）。

#### F2b. `syllable` 宣言

```hu
syllable lang_proto {
  template:       (C) V (C) (C)
  nucleus:        V
  onset_max:      1
  coda_max:       2
  onset_priority: max | min
  unknown:        ignore | skip | warn | error  # 不明文字の扱い、default は warn
  unknown_overrides: {                          # 任意：特定文字の例外的扱い
    " ": skip
  }
}
```

- `template` は phoneme 参照（`C`、`V`）または class 名で表現された音節形状。

**強勢・音節重量について**：これらを文法カテゴリーとして 1st class 化する代わりに、**表音要素（phoneme）として通常のインベントリに含める**設計を推奨する。マクローン付き母音を heavy として、アキュート付き母音を強勢ありとして、`phoneme` 宣言に列挙すればよい。phonrule は通常の rewrite で条件参照でき、最終 render で見せたくない場合は剥がす phonrule を最後に当てる。

例：

```hu
phoneme heavy_vowel { "ā", "ē", "ī", "ō" }
phoneme light_vowel { "a", "e", "i", "o" }
phoneme V { heavy_vowel; light_vowel }

# 重音節でのみ起こる変化
phonrule X {
  syllable: lang_proto
  voiced -> voiceless / heavy_vowel σ[ _ ]σ $
}

# render 前にマクローンを剥がす
phonrule normalize {
  "ā" -> "a"   # 以下同様
}
```

この方針により、新しい文法カテゴリー（stress 軸、weight 軸など）を追加する必要がなく、phoneme ＋ phonrule の組み合わせで完結する。conlang 設計者が好む notation（`á` / `ā` / `a:` / `a1` 等）も自由に選べる。

**`unknown` モード**：

| Mode | 通知 | 挙動 |
|------|------|------|
| `ignore` | 静か | 透過：不明文字は音節境界計算に無関係、現在音節を継続 |
| `skip` | 静か | 区切る：不明文字を音節境界として扱い、直後から新音節 |
| `warn` | 警告ログ | `skip` 相当（既定で安全側） |
| `error` | コンパイル時エラーで停止 | — |

**内部マーカー（`\0`、compose の `+`）は hubullu 側で syllabify 入力前に自動 strip**：ユーザーは意識不要。`unknown_overrides:` は外側 escape hatch（普通は不要）。

#### F2c. phonrule 内の新 context 要素

> **【DEPRECATED — v2 機能 M で置換済み】** 以下の `σ[` `]σ` `σ#N` 系の
> 非 ASCII トークンは v2 proposal (`phonrule-extension-v2.md`) の機能 M で
> **完全削除**され、ASCII の macro 構文に移行した。現行コンパイラは σ 構文を
> parse エラーにする。移行表（v2 §2.3 より抜粋）:
>
> | 旧 (σ) | 新 (macro) |
> |---|---|
> | `σ[`（単独） | `%syl<head>%` |
> | `]σ`（単独） | `%syl<tail>%` |
> | `σ[ X _ Y ]σ` | `%syl[ X _ Y ]%` |
> | `σ#1` | `%syl<#1>%` |
> | `σ#3+` | `%syl<#{3..}>%` |
> | `σ#2-4` | `%syl<#{2..4}>%` |
>
> 評価モデル（lazy syllabification）と `syllable: NAME` フィールド必須の判断は
> M でも不変。`%syl<head>%` / `%syl<tail>%` / `%syl[...]%` は σ 時代の
> セマンティクスをそのまま引き継ぐ。`%syl<#N>%`（音節インデックス参照）は
> M では parse のみ対応で、評価は F7 で実装予定。
>
> 以下は歴史的記録としての原文（v1 当時の仕様）:

```
context_elem = ... | "σ[" | "]σ" | "σ"
```

- `σ[` ＝ 音節開始境界、`]σ` ＝ 音節終了境界。`^`/`$` と同列。
- `σ[ X _ ]σ` で「現在音節内」マッチ可。
- 音節重量条件は phoneme class と境界の組み合わせで表現（heavy = "coda 持つ音節" は `]σ` 直前に C があるかで判定）。

**評価モデル**：phonrule 評価エントリで「文字列に対し on-demand syllabify → 各 syllable boundary 位置の bitset を作成 → match 時に位置をルックアップ」の **lazy syllabification**。phonrule 適用で文字列が変わるたびに syllabify をやり直す。

判断：**`phonrule` block に `syllable: NAME` フィールドを追加**して syllable rule を明示参照。複数 syllable 宣言の曖昧性回避。省略時は σ context 要素を使えない。

### F3. `phonrule` 合成

**構文**

```
phonrule daughter_a {
  display: { en: "Daughter A" }       # 任意
  derived_from: proto                 # 任意・informational only
  apply oe_to_e                       # 単独で apply 文を許可
  apply final_drop
  apply coda_devoicing
  # 加えて従来通り class/map/rewrite も書ける（混在可）
}
```

**セマンティクス**

- phonrule body に `apply IDENT` 文を許容（宣言順に意味あり）。
- 評価時：`apply_phonrule(input, P)` の中で、P の `apply` 文を順次解決して `apply_phonrule(_, Q)` を呼ぶ。P 自身の class/map/rules も「自分が含む `apply` を全部適用した後」最後に走らせる（or 先に走らせる？ → **`apply` 文は宣言順に処理、混在時も書いた位置どおりに評価**。新 AST `PhonRuleBody = Vec<PhonItem>` で `Class/Map/Rewrite/Apply` を順序保存）。
- **循環検出**：フェーズ2の `validate_phonrules` で各 phonrule を depth-first 訪問、訪問中スタックに同名があればエラー。
- `display`・`derived_from` フィールドは inflection と同様に純粋メタ情報。SQLite には別テーブル（後述）で永続化。

### F4. CLI `-e` フラグ

**構文**

```
hubullu render <input.hut> [-e <code>]... [--huc <path>]
```

- `-e` は**繰り返し指定可**（`-e A -e B` → `A; B` と等価）。
- `<code>` は完全な `.hut` 構文の文字列。`@use`・`@apply`・インライン phonrule 定義（`phonrule q { ... }`）・トークン列を含められる。
- `@file <path>` 形式の sugar：`-e "@file:./extra.hut"` で「先頭が `@file:`」なら指定パスから読み込む（curl 風）。
- **評価コンテキスト**：`-e` 文字列は **元の `.hut` の末尾に append** されたかのように扱われる。すなわち同一 `@reference` スコープ・同一 file-id（仮想ファイル `<eval>` を発行）で並列パース→merge。
- file-level `@apply` を `-e` で追加した場合：**元ファイルの apply chain の末尾に append**（直感的な「重ねがけ」）。明示的に「置換」したい場合は別フラグ（将来拡張）。
- 元 `.hut` に存在しない phonrule を `-e` 内で `phonrule q { ... }` 定義することも可能。

### F5. セミコロン区切り

- lexer に `TokenKind::Semicolon` を追加（`;`）。
- **statement 境界限定**：parser の **statement loop の skip 位置でのみ** セミコロンを許容する（top-level の宣言間、block 内の statement 間など）。
- **式・field・括弧内では `;` は不正トークンとしてエラー**。これにより typo や意図しない混在をマスクしない。
- 既存テキストへの影響ゼロ（既存ファイルに `;` は無い）。

例：

```hu
phonrule q { "œ" -> "e" }; @apply q; @apply r        # OK：statement 境界
phonrule q { ; class V = [...] ; }                    # NG：block 内 statement 間も将来許す余地はあるが phase 1 では NG
phonrule q { "œ" -> ; "e" }                           # NG：式内
```

- 「statement とは何か」を厳密に定義：top-level 宣言（phonrule, syllable, phoneme, inflection, entry, tagaxis, @extend, @use, @reference, @export, @apply, @render）の各単位を1 statement とする。
- `.hut` の token 列内は statement ではない（あくまでトークン列）ので `;` は使えない。

---

## 3. 実装計画

### 依存関係グラフ

```
F5 (semicolon) ─────────────┐
                            ├─→ F4 (CLI -e)
F1 (@apply / phon_call) ────┘
F3 (phonrule composition)  独立
F2 (syllable rule)         独立（が F3 と並走しても良い）
```

### 順序・労力

| Step | 機能 | 労力 | 内容 |
|------|------|------|------|
| 1 | F5: `;` 受容 | 小 | `TokenKind::Semicolon` 追加・lexer 1分岐・parser に `skip_semicolons()` ヘルパ、各 statement loop に挿入。テスト：`;` 入りファイルが既存と同等に compile/render。**ここで release 切れる**。|
| 2 | F3: phonrule 合成 | 中 | (a) `PhonRule` AST を `Vec<PhonItem>` ベースに刷新（互換のため Vec を 3つに分けたままにし、別 `Vec<PhonApply>` を追加でも可）。(b) parser に `apply IDENT` body 文を追加、`display:`/`derived_from:` フィールドも追加。(c) `phonrule_eval::apply_phonrule` を再帰化、resolver 経由で nested rule を解決。(d) phase2 で循環検出 DFS。テスト：単純合成・循環エラー・既存 phonrule の動作不変。**ここで release 切れる**。|
| 3 | F1a: `.hut` `@use` 受容 | 小 | `.hut` parser に `@use` を解禁（既存 `parse_import` 流用）。phase1/2 の symbol テーブルが `.hut` でも `@use` 経由の phonrule を見えるように。 |
| 4 | F1b: `@apply` directive | 中 | (a) `HutFile` に `apply_chain: Vec<Ident>` 追加。(b) parser で `@apply` を `@reference` と同じ位置で消化。(c) `render::resolve` で「Glue で連結された極大連鎖」を音韻語と認識、`apply_phonrule` を chain 順に通す。 |
| 5 | F1c: インライン `phon_call` & `@apply` block | 中 | parser に Token 種 `PhonCall { rule: Ident, inner: Vec<Token> }`、`ApplyBlock { rule: Ident, inner: Vec<Token> }` を追加。resolve 時にスタックを保持。**ここで release 切れる**。|
| 6 | F4: CLI `-e` | 小〜中 | `clap` で `-e <String>` を `Vec<String>` に。`parse_hut` を「2つの source を1 file-id ずつパースして HutFile を merge」する API に拡張（または string concat で1 file 扱い）。`@file:` sugar。 |
| 7a | F2a: `phoneme` 宣言 | 中 | (a) `Phoneme` AST/parser/symbol（top-level）。block 内にリテラル＋参照混在。(b) phase2 で循環参照 DFS 検出、union 展開して各 phoneme の終端集合を確定。(c) longest-match-first tokenize ヘルパ。(d) phonrule body の class 評価で phoneme 参照を解決可能に。テスト：単純宣言、union、循環エラー、multigraph 解決。|
| 7b | F2b: `syllable` 宣言 ＋ unknown 処理 | 中 | (a) `Syllable` AST/parser/symbol（top-level）。(b) template, nucleus, onset_priority, unknown, unknown_overrides フィールド。(c) syllabify アルゴリズム（template ＋ onset_priority ＋ nucleus phoneme からの greedy 解析、unknown 文字を mode に従って処理）。(d) 内部マーカー `\0` `+` を syllabify 前に strip するヘルパ。テスト：CV / CVC 言語、ignore/skip/warn/error 各モード。|
| 7c | F2c: phonrule の σ-aware context | 中 | (a) `phonrule` に `syllable: NAME` フィールド追加。(b) phonrule_eval に「σ-aware context match」：context elem に `SylStart/SylEnd` を追加、context check で boundary bitset を参照。(c) lazy syllabification（phonrule 適用ごとに syllabify）。(d) SQLite 出力テーブル `phonemes` `syllables` 追加。テスト：coda devoicing、open-final drop、heavy/light 条件のサンプル。|

### テスト戦略

- **F5**：lexer unit ＋ 既存全テストが `;` 注入版でも通る fixture 1個。
- **F3**：phonrule_eval の unit test に「ネスト phonrule」「3段チェイン」「循環エラー」。
- **F1**：`tests/integration.rs` に `.hut` end-to-end ケース。元ファイル ＋ 期待出力で比較（小規模 conlang fixture）。
- **F4**：CLI integration test を `assert_cmd` or 既存 framework で `-e` 経由を網羅。
- **F2**：phonrule_eval unit に「ハードコード syllabification」ケース、加えて end-to-end で「コーダ無声化」が音節境界依存で正しく動くこと。

### ブレークポイント（commit/release できる切れ目）

1. F5 完了後（既存挙動非破壊）
2. F3 完了後（phonrule 機能拡張のみ、`.hut` 未変更）
3. F1 完了後（`.hut` 拡張、a priori プロジェクトで使えるようになる最小有用点）
4. F4 完了後（CLI 完備）
5. F2 完了後（高度な phonrule）

---

## 4. 解決済みの判断（全項目）

すべての設計判断が確定。実装着手可能な状態。

**F1（`.hut` の `@apply` と インライン phonrule 関数）**

- `~` の意味付け：膠着マーカー (agglutination boundary) として再解釈。既存セマンティクスを「変更」せず「本来そうだった」と位置付ける。新マーカー不要。
- `@apply` block：**許可**。ネスト評価器を実装し、外側 active apply スタックに push/pop する形で評価。インライン `phon_call` との優先関係（最内 phon_call が優先）と整合。

**F2（`phoneme` 宣言 ＋ `syllable` rule）**

- 音素集合の表現：top-level `phoneme NAME { ... }` 宣言を導入。phonrule の `class` とは併用（phoneme=global、class=phonrule-local）。multigraph は longest-match-first。
- 不明文字の扱い：4モード（`ignore` / `skip` / `warn` / `error`）、default は `warn`。内部マーカー（`\0`, `+`）は hubullu 側で syllabify 入力前に自動 strip。`unknown_overrides` は escape hatch。
- 強勢・音節重量：1st class 化せず、**表音要素として通常の phoneme インベントリに含める**設計を推奨。マクローンやアキュートを heavy/stressed のマーカーとして扱い、必要なら最終 phonrule で剥がす。
- syllable 参照方式：`phonrule { syllable: NAME ... }` で**明示参照を必須**。複数 syllable 宣言の曖昧性回避。省略時は σ context 要素を使えない。

**F3（`phonrule` 合成）**

- body 内構文：`apply IDENT` 文と inline rewrite rule の**混在を許可**。宣言順に評価。新 AST `PhonRuleBody = Vec<PhonItem>` で `Class/Map/Rewrite/Apply` を順序保存。
- 循環検出：phase2 で DFS 訪問、訪問中スタックに同名があればエラー。
- `derived_from` フィールド：**informational only**（純粋メタ情報）。SQLite には永続化するが、コンパイラ挙動に影響しない。将来「proto との diff チェック」等の意味付けを後付け可能な余地は残す。

**F4（CLI `-e` フラグ）**

- 命名・繰り返し：`-e <code>` を**繰り返し指定可**（`-e A -e B` → `A; B` と等価）。`@file:` sugar 形式で外部ファイル読み込み可。
- 評価コンテキスト：`-e` 文字列は元 `.hut` の末尾に append されたかのように扱う。同一 `@reference` スコープ・仮想ファイル `<eval>` を発行。
- `@apply` の重ねがけ：`-e` で追加した file-level `@apply` は**元 `.hut` の chain に append**。明示置換の必要が出てきたら別フラグ（将来拡張）。

**F5（セミコロン区切り）**

- 位置：**statement 境界限定**。式・field・括弧内では不正トークンとしてエラー。typo マスキング回避。
- 影響範囲：既存ファイルに `;` は無いため後方互換完全。

---

## Critical Files for Implementation

- `/Users/csakai/repos/anl-tools/hubullu/src/parser.rs`
- `/Users/csakai/repos/anl-tools/hubullu/src/phonrule_eval.rs`
- `/Users/csakai/repos/anl-tools/hubullu/src/render.rs`
- `/Users/csakai/repos/anl-tools/hubullu/src/ast.rs`
- `/Users/csakai/repos/anl-tools/hubullu/src/main.rs`
