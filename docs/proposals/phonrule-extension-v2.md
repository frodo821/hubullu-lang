# Phonrule Extension v2: 音節位置を扱うルール記述

v1 (`phonrule-extension.md`) で F1〜F5 を実装し、`syllable` 宣言と `σ[`/`]σ` 境界マーカーは入った。が、実際に子孫言語の音韻変化を書こうとすると以下のような頻出パターンが書きにくい / 書けないことが分かった。加えて、`σ` 系の非 ASCII トークンは編集・grep・IME 切替の観点で運用しづらい。本 proposal は **3 つの機能拡張 + macro 構文への完全移行** を提案する。

a priori 祖語プロジェクト (`/Users/csakai/repos/writing/a_priori/run_01/`) でのワークフロー要請が起点。

## 概要

| ID | 機能 | 主目的 |
|----|------|--------|
| F6 | context elem の **quantifier** (`*` `+` `?` `{n,m}`) + wildcard `.` | 開/閉音節判定、複数 segment 範囲の look-ahead |
| F7 | **syllable index 参照** (`%syl<#1>%` `%syl<#-1>%` `%syl<#{3..}>%`) | 「語頭/語末音節」「3 音節目以降」を明示表現 |
| F8 | **LHS range rewrite** (左辺にも quantifier) | 「3 音節目以降を脱落」を 1 ルールで |
| **M** | **macro 構文 `%name<spec>[seq]%` への完全移行**、σ 系 deprecate | ASCII のみで運用、拡張性、grep 容易 |

F8 は実装コストが大きいため、F6 + F7 + M のみで主要ユースケースを賄えるか別途検討する余地あり。

---

## 1. 動機

### 1.1 開音節 vs 閉音節

「開音節末の長母音化」は古典的な音韻変化:

```
V -> Vː / %syl[ C* V _ ]%   # 開音節末（V の後に C なしで syl 終端）
```

現状は context elem に quantifier がないので `C*` が書けず、`%syl[ V _ ]%` か `%syl[ C V _ ]%` のように onset の有無で場合分けして書く必要があり、CV / CCV / V を網羅できない。

### 1.2 音節位置による条件

「最終音節の有声化」「語頭音節の保護」など:

```
voiceless -> voiced / %syl<#-1>% _   # 最終音節内
ə -> a / %syl<#1>% _                 # 語頭音節内
```

現状は境界しか取れないので、「語頭から数えて何番目の音節か」を表現できない。最終音節を指定するには「以降に音節境界がない」を後方文脈で書く必要があり、それも `]σ $` までしか表現できず曖昧。

### 1.3 第n音節以降の脱落

自然言語によくある変化（インド・ヨーロッパ系での音節脱落、強勢パターン由来の中音節弱化など）:

```
# 3 音節目以降が脱落:
%syl[ . * ]% -> ∅ / %syl<#{3..}>% _
```

現状は LHS が単一 segment なので、音節範囲を一括して書き換える手段がない。位置ごとに `_ -> ∅` で各 segment を消す回避策はあるが、削除によって音節インデックスがズレるので lazy syllabification との相互作用で誤動作する。

### 1.4 多 segment look-ahead

「VCV パターンで真ん中の C が有声化」:

```
voiceless -> voiced / V _ V       # 既に書ける
voiceless -> voiced / V V* _ V V* # V の連鎖を許容したい (書けない)
```

現状は context が固定長 segment 列なので、可変長 look-ahead が表現不能。

### 1.5 σ 系トークンの運用コスト

`σ[ ]σ σ#` はいずれも ASCII 外。`σ` 自体は U+03C3 (Greek Small Letter Sigma) で IME 切替か special char 入力が必要。`σ#-2-` のように suffix 修飾子 `+` `-` を導入すると、`-` の役割 (負号 / 範囲区切り / 「以前」修飾子) が文脈で 3 種に分裂し parser も読み手も混乱する。

---

## 2. macro 構文

### 2.1 文法

```
macro     = "%" name ( "<" spec ">" )? ( "[" sequence "]" )? "%"
name      = IDENT
spec      = num_spec | name_spec
num_spec  = "#" INTEGER
          | "#" "{" range "}"
range     = bound? ".." bound?      # 両端 inclusive、片側または両側省略可
bound     = INTEGER                  # 負数で語末から (-1 = 末尾)
name_spec = IDENT                    # macro ごとに定義済みのキーワード (例: head, tail)
sequence  = context_elem*
```

### 2.2 設計判断

- **数値・範囲は `#` 前置必須**: `<#1>` `<#{1..4}>` `<#-2>`。`#` なしで `<-2>` と書くと曖昧 (負数なのか修飾子なのか不明) なので parse エラー
- **範囲は両端 inclusive**: `#{1..4}` = 1, 2, 3, 4 (Rust の `..` ではなく `..=` 相当)。conlang author に直感的
- **片側省略可**: `#{3..}` (3 番目以降)、`#{..-2}` (2 番目末以前)、`#{..}` (全部、ただし冗長なので非推奨)
- **name 部分は予約語**: macro ごとに固定のキーワードを定義。`<head>` `<tail>` などは syl macro 内のみ
- **空 macro (`%syl%`) は parse エラー**: spec も seq もない場合は不正トークン

### 2.3 σ 系からの移行表

| 旧 (σ) | 新 (macro) | 意味 |
|---|---|---|
| `σ[` (単独) | `%syl<head>%` | 音節開始境界 anchor (zero-width) |
| `]σ` (単独) | `%syl<tail>%` | 音節終了境界 anchor (zero-width) |
| `σ[ X _ Y ]σ` | `%syl[ X _ Y ]%` | 音節内容ブロック |
| `σ#1` | `%syl<#1>%` | 語頭音節内 |
| `σ#-1` | `%syl<#-1>%` | 語末音節内 |
| `σ#3+` | `%syl<#{3..}>%` | 第 3 音節以降 |
| `σ#-2-` | `%syl<#{..-2}>%` | 語末から 2 番目以前 |
| `σ#2-4` | `%syl<#{2..4}>%` | 第 2 〜 第 4 音節 |
| (新) | `%syl<#{1..-1}>%` | 全音節 (実質常に真) |
| (新) | `%syl<#{-3..-1}>%` | 最後 3 音節 |

`σ#n+` (suffix `+`)、`σ#n-` (suffix `-`) は range 表現に統一されることで、suffix `-` と負号の衝突が解消される。

### 2.4 拡張余地 (本 proposal では実装しない)

- `%word<head>%` / `%word<tail>%` (= `^` / `$` の置換) — 既存 `^` `$` は伝統的表記として残す案
- `%foot[...]%` `%foot<#N>%` — 強勢足
- `%stress<...>%` — 強勢条件
- `%mora<...>%` — モーラ位置

---

## 3. 現状調査サマリー

**`src/phonrule_eval.rs`** — `check_context` は LHS マッチ位置 `pos` から左右に context elem を 1 つずつ消費して照合する **線形 walk**。各 context elem は `^` `$` `SylStart` `SylEnd` `Class` `Literal` のいずれかで、いずれも **0 幅 (anchor) または 1 segment 幅** のみ。可変長マッチの概念は存在しない。

**`src/ast.rs`** — `ContextElem` enum:
```rust
enum ContextElem {
    WordStart, WordEnd,
    SylStart, SylEnd,
    Class(Ident),
    Literal(String),
}
```

**LHS** — `RewriteRule.lhs: RewriteLhs` で `Class(Ident)` または `Literal(String)`。**単一 segment 固定**。

**syllabify** — F2c で各 phonrule 適用前に呼び、`SyllableBoundaries { starts: Vec<usize>, ends: Vec<usize> }` を構築。**syllable index 概念は持たない** (n 番目の音節の position を求めるには starts[n-1] / ends[n-1] でアクセスすれば良いので軽量な拡張で済む)。

**Rewrite loop** — collapse 収束まで反復: 1 segment 単位で LHS を試行し、最初の match で置換。**多 segment 同時置換は不可**。

---

## 4. 機能仕様

### F6. context elem の quantifier + wildcard

#### 構文

```
context_elem = ... | quant_elem | "." ;
quant_elem   = base_elem quantifier ;
base_elem    = class | literal | "." | syl_block ;
syl_block    = "%syl[" context_elem+ "]%" ;
quantifier   = "*" | "+" | "?" | "{" NUMBER "}" | "{" NUMBER "," NUMBER? "}" ;
```

- `*` 0 個以上、`+` 1 個以上、`?` 0 or 1、`{n}` 厳密、`{n,m}` 範囲、`{n,}` n 以上
- **wildcard `.`**: 任意の 1 phoneme (境界 anchor `%syl<head>%` / `%syl<tail>%` 廃止後の代替)
- `base_elem` には phoneme class / literal / `.` / `%syl[ ... ]%` を許可 (`^` `$` `%syl<head>%` `%syl<tail>%` の anchor は quantifier 不可)
- `%syl[ ]%` の中に再帰的に context_elem を書ける

#### セマンティクス

- **greedy backtracking** マッチ。NFA を想定 (現行 phonrule の収束ループ性能を著しく悪化させないため、context 長は実装上 50 segment 上限など)
- `*` `+` `?` の組み合わせで「open syllable: `%syl[ C* V ]%`」「closed syllable: `%syl[ C* V C+ ]%`」を表現可能
- 量化対象が syl block の場合: `(%syl[ ... ]%){2,}` で「条件を満たす音節が 2 つ以上連続」
- 任意 phoneme `.` により「任意の音節末尾」: `%syl[ .* _ ]%`

#### 例

```hu
phonrule open_lengthening {
  syllable: lang_proto
  V -> Vː / %syl[ C* V _ ]%              # 開音節末で長母音化
}

phonrule final_devoicing {
  syllable: lang_proto
  voiced -> voiceless / _ %syl<tail>% $  # 語末音節 coda (head/tail anchor 利用)
  # または:
  voiced -> voiceless / _ %syl[ .* _ ]% $  # wildcard ベース
}

phonrule any_syl_final {
  syllable: lang_proto
  b -> p / _ %syl<tail>%                 # 任意の音節末 (`]σ` 相当)
}
```

#### エッジケース

- `C*` が 0 個でマッチする場合と隣接 anchor の関係: `^ C* V _` は語頭 (子音 0 個以上 + V を経て本位置)
- ネストした syl block: `%syl[ V %syl[ C ]% _ ]%` のような奇妙なネストは禁止 (syllabify 結果は flat なので意味なし)
- greedy vs lazy 切替: 当面 greedy のみ (lazy `*?` `+?` は将来拡張)
- wildcard `.` と phoneme class の衝突: `.` は予約トークン (識別子としては不可)。`phoneme . { ... }` は parse エラー

### F7. syllable index 参照 (`%syl<#...>%`)

#### 構文

```
context_elem = ... | syl_index_anchor ;
syl_index_anchor = "%syl<" num_spec ">%" ;
num_spec   = "#" INTEGER | "#{" range "}" ;
range      = bound? ".." bound? ;
bound      = INTEGER                       # 正で語頭から (1-indexed)、負で語末から (-1)
```

- 正の数: 語頭から数える (1-indexed)
- 負の数: 語末から数える (-1 = 末尾音節)
- **範囲は両端 inclusive**

#### セマンティクス

- `%syl<#n>%` は **zero-width anchor**: 「現在の cursor position が n 番目の音節の内部にある」を真とする
- 評価: syllabify 結果の `SyllableBoundaries` から各 `(start_i, end_i)` を取り、`start_i ≤ pos < end_i` の `i` を逆引きする。`i` (1-indexed) と spec を照合
- `%syl<#-1>%` は `i == syllable_count`、`%syl<#-2>%` は `i == syllable_count - 1` など
- 範囲 `#{a..b}`: 正規化後の `a ≤ i ≤ b` で真
- 空音節 (たとえば子音のみの孤立トークン) のハンドリング: syllabify 結果に依存。シンボルが音節を構成しなければ無音節とみなし `%syl<#n>%` は常に false

#### 例

```hu
phonrule final_devoicing {
  syllable: lang_proto
  voiced -> voiceless / %syl<#-1>% _      # 最終音節内のすべての位置
}

phonrule head_protection {
  syllable: lang_proto
  ə -> a / %syl<#1>% _                    # 語頭音節内では ə を a に保持
}

phonrule late_loss {
  syllable: lang_proto
  V -> ∅ / %syl<#{3..}>% _                # 3 音節目以降の母音を脱落
}

phonrule middle_weakening {
  syllable: lang_proto
  voiceless -> voiced / %syl<#{2..-2}>% _ # 中間音節 (語頭・語末以外) で有声化
}
```

#### エッジケース

- 単音節語: `%syl<#1>%` と `%syl<#-1>%` の両方が真。`%syl<#{1..1}>%` も真
- `%syl<#0>%` は不正 (1-indexed が原則)。コンパイル時エラー
- `%syl<#{a..b}>%` で `a > b` (正規化後) は範囲が空 = 常に false。compile-time warning か
- `%syl<#{n..}>%` で n が syllable count を超える場合は常に false (no-op)
- `%syl<` を `syllable:` フィールドなしの phonrule で使うと F2c と同様コンパイル時エラー

### F8. LHS range rewrite (オプショナル)

#### 構文

```
rewrite_rule = lhs "->" rhs ("/" context)? ;
lhs          = base_elem quantifier?         # 既存の単一 segment に加え quantifier 許可
             | syl_block quantifier?         # %syl[...]% を LHS に
             ;
rhs          = base_elem | literal | "∅"   # 既存通り、削除は ∅ (or 空文字列)
             ;
```

- LHS に quantifier を許可
- LHS が `%syl[ ... ]%` の場合: マッチした音節範囲全体を rhs で置換

#### セマンティクス

- マッチした **range** 全体が 1 回の置換単位として rhs に変わる
- range 内に複数音節が含まれる場合: それらすべてを rhs 1 文字 (or 文字列) に変える
- 収束ループ: 1 回の rewrite で range を消費する。ループは新しい syllabify 結果に基づき再走

#### 例

```hu
phonrule late_syllable_loss {
  syllable: lang_proto
  (%syl[ C* V C* ]%)+ -> ∅ / %syl<#3>% _   # 3 音節目から始まる連続音節をすべて削除
}

phonrule cluster_reduction {
  syllable: lang_proto
  C{2,} -> C / %syl[ _ V ]%                 # 音節 onset の子音連続を 1 つに (実は具体化が必要)
}
```

#### 実装上の課題

- LHS が range だと、rewrite engine が完全に正規表現エンジンになる
- collapsed convergence ループの停止性: greedy なら毎回 fix point に収束するが、テストケースを慎重に
- syllable index 参照 (`%syl<#3>% _`) が **削除前のインデックス** か **削除後のインデックス** か明確化が必要 → **削除前** で評価 (lazy syllabify が rewrite 前にチェックされる)
- パフォーマンス: 各 phonrule で正規表現エンジンを起動するコスト。`regex-automata` crate などの活用検討

### M. macro 構文への完全移行

#### 移行範囲

- F2c で導入した `σ[` `]σ` `σ#N` `σ#N+` `σ#N-` `σ#N-M` トークン群を **削除**
- すべて §2.3 の表に従い `%syl<...>%` `%syl[...]%` 形に置換
- lexer から `σ` 関連トークン認識を除去
- parser の context elem パーサを macro 文法に刷新
- 既存 hubullu テスト (`tests/integration_sigma.rs` 等) を新構文に書き換え
- proposal 内例 (`phonrule-extension.md` の F2c 部分) も更新

#### 互換性

- v1 で書かれた既存 phonrule (σ 構文未使用) は **完全互換**
- F2c の σ 構文を使った既存ファイルは parse エラーになる (現状 a priori プロジェクトでは未使用なので影響なし)
- main 上の `tests/integration_sigma.rs` は v2 実装時に書き換え必須

---

## 5. 実装計画

### 5.1 依存関係

```
M (macro 構文) ────────┐
                       ├─→ F6 (context quantifier) ─┐
F7 (syl index)    ─────┘                            ├─→ F8 (LHS quantifier)
                                                    │
                                                    └─→ (現行 σ 構文の削除)
```

M は最初に着手 (lexer / parser の基盤刷新)。F6 と F7 は M の上に乗る独立な拡張。F8 は F6 + F7 の上。

### 5.2 段階別

| Step | 機能 | 労力 | 内容 |
|------|------|------|------|
| 1 | M: macro lexer + parser | 中 | lexer に `%` `<` `>` macro トークン認識を追加。parser に macro 文法を実装。`%syl[...]%` `%syl<spec>%` `%syl<#n>%` `%syl<#{n..m}>%` を読める状態にする。AST はまだ既存 `ContextElem::{SylStart, SylEnd}` のまま流用 (head/tail のみ移行) |
| 2 | F2c の σ 構文を削除 | 小 | lexer から σ トークン認識を除去。`tests/integration_sigma.rs` を `tests/integration_syl_macro.rs` (相当) にリネーム+書き換え。release 切れ目 |
| 3 | F7: syl index | 小〜中 | `ContextElem::SylIndex(SylSpec)` 追加。parser で `%syl<#...>%` の数値・範囲をパース。phonrule_eval で zero-width anchor として評価。`syllable:` 必須の検証は F2c と同様。release 切れ目 |
| 4 | F6: 量化子 + wildcard | 中 | `ContextElem` を `Pattern` という新 AST に拡張 (`Atom` + `Quantifier`)。NFA / バックトラッキング match engine を実装。greedy のみ。wildcard `.` 追加。phonrule_eval の `check_context` を全面刷新。release 切れ目 |
| 5 | F8: LHS 量化子 | 大 | LHS パーサ拡張。rewrite engine を「単一 segment + 単一置換」から「range match + range 置換」に。convergence ループ・syllabify との相互作用テスト多数。release 切れ目 |

### 5.3 ブレークポイント

1. M 完了後 (新構文で書ける、旧構文書き換え済、既存挙動は同等)
2. F7 完了後 (語頭/語末音節指定が可能、最も実用度の高い小さな追加)
3. F6 完了後 (開/閉音節の表現が可能、context が表現力的に SPE notation 並みに)
4. F8 完了後 (フル regex 化、a priori で「音節脱落」「クラスター簡略化」が書ける)

---

## 6. 主要 ユースケースとの対応表

a priori 子孫言語派生で想定する変化:

| 変化 | v1 のみ | M+F7 | M+F6 | M+F6+F7 | M+F6+F7+F8 |
|---|:-:|:-:|:-:|:-:|:-:|
| 単純置換 (`ə → e`) | ✅ | ✅ | ✅ | ✅ | ✅ |
| 語末有声 → 無声 (`b → p / _ $`) | ✅ | ✅ | ✅ | ✅ | ✅ |
| 音節末 (`b → p / _ %syl<tail>%`) | ✅ | ✅ | ✅ | ✅ | ✅ |
| 開音節末長母音化 | ❌ | ❌ | ✅ | ✅ | ✅ |
| 閉音節弱化 | ❌ | ❌ | ✅ | ✅ | ✅ |
| 最終音節有声化 | ❌ | ✅ | ❌ | ✅ | ✅ |
| 語頭音節保護 | ❌ | ✅ | ❌ | ✅ | ✅ |
| 中間音節弱化 | ❌ | ✅ | ❌ | ✅ | ✅ |
| 第n音節以降脱落 (位置ごと) | ❌ | ✅ | ❌ | ✅ | ✅ |
| 第n音節以降脱落 (range 1 ルール) | ❌ | ❌ | ❌ | ❌ | ✅ |
| 子音連続簡略化 (range) | ❌ | ❌ | ❌ | ❌ | ✅ |

---

## 7. 解決すべき設計判断

### 7.1 量化対象

- `^` `$` `%syl<head>%` `%syl<tail>%` などの anchor は量化不可で確定
- phoneme class (`C`, `V`, `vowels_front`) は量化可で確定
- literal (`"a"`, `"bː"`) は量化可で確定
- wildcard `.` は量化可で確定
- `%syl[ ... ]%` block を量化対象にすると複雑化。**最初は禁止し、F8 と同時に解禁** が安全

### 7.2 greedy vs lazy

phonrule の収束ループは greedy で動作している。**lazy 量化子 (`*?` `+?`) は v2 では導入せず、必要になったら v3 で**。

### 7.3 syl index と量化の併用

`%syl<#{3..}>% %syl[ C* V ]%` のように index と量化を併用するケース: 「3 音節目以降にある開音節」を表現。これは F6 + F7 の自然な合成として動くべき。**特別な処理なし**。

### 7.4 LHS 量化のセマンティクス

- `C+ -> ∅` は「1 つ以上の C を全部削除」か「1 つの C を削除して再評価」か → **range 全体を削除** が直感的
- `C{2,} -> C` は「2 つ以上の連続 C を 1 つに」 → range 削除＋単 C 挿入として実装

### 7.5 macro 名空間の予約

- 当面 `syl` のみ予約。`word` `foot` `stress` `mora` は将来拡張で予約
- ユーザー定義 macro は v2 では非対応 (v3 以降検討)

---

## Critical Files for Implementation

- `/Users/csakai/repos/anl-tools/hubullu/src/lexer.rs` — `%` `<` `>` トークン、σ 削除
- `/Users/csakai/repos/anl-tools/hubullu/src/token.rs` — macro 用 TokenKind 追加、`SylStart/End/Index` 等を整理
- `/Users/csakai/repos/anl-tools/hubullu/src/ast.rs` — ContextElem 拡張、SylIndex/SylHead/SylTail variant、Pattern struct
- `/Users/csakai/repos/anl-tools/hubullu/src/parser.rs` — macro パーサ、quantifier パーサ
- `/Users/csakai/repos/anl-tools/hubullu/src/phonrule_eval.rs` — match engine の刷新、syllable index 逆引き
- `/Users/csakai/repos/anl-tools/hubullu/src/phase2.rs` — `syllable:` 必須検証の拡張 (F7 でも必須)
- `/Users/csakai/repos/anl-tools/hubullu/src/emit_sqlite.rs` — context パターンのシリアライゼーション形式変更
- `/Users/csakai/repos/anl-tools/hubullu/tests/integration_sigma.rs` — 新構文へ書き換え (リネーム検討)
- `/Users/csakai/repos/anl-tools/hubullu/docs/proposals/phonrule-extension.md` — F2c セクションを新構文に追記 (deprecated 注記)
