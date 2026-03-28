# LexDSL — Artificial Natural Language Dictionary Format Specification

## 概要

LexDSLは人工自然言語の辞書を記述するためのドメイン固有言語。  
設計原則は以下の通り：

- **言語中立性**：ラテン語・バスク語・日本語・中国語など任意の形態論的型に対応
- **宣言とロジックの分離**：エントリはデータのみを持ち、形生成ロジックはプロファイルに閉じる
- **コンパイル前の整合性検証**：リンク切れ・活用形の未定義を構文解析時に検出
- **綴り変更の安全性**：IDで参照し、headwordの変更が波及しない構造

---

## アーキテクチャ

```
ソースファイル群 (.hu)
  profile.hu          # タグ軸・活用クラス・形態クラス定義
  entries/**.hu       # エントリ定義

          ↓ コンパイラ

dictionary.sqlite      # 生成物（git管理不要）
  entries              # エントリ本体・FTS用意味テキスト
  forms                # 活用形 → エントリIDの逆引き
  links                # エントリ間リンクグラフ
  broken_refs          # 整合性チェック結果
```

---

## ファイル構成・プロジェクト構成

エントリポイントとなるファイルをコンパイラに渡す。  
`@use` のパス解決はエントリポイントファイルからの相対パス。  
ディレクトリ構造・ファイル命名・1ファイルあたりのエントリ数に規約はない。

```
my-lang/
  profile.hu
  entries/
    verbs.hu
    nouns.hu
    ...
  stdlib/              # 標準ライブラリ（任意）
    universal_pos.hu
    verbal_categories.hu
```

---

## `@use` / `@reference` — インポート構文

`@use` と `@reference` は同一の構文を共有する。

```ebnf
import             = "@" ("use" | "reference") import_target "from" string_literal ;
import_target      = "*" ("as" ident)?
                   | import_ident_list ;
import_ident_list  = "(" import_ident_entries ")"
                   | import_ident_entries ;
import_ident_entries = (import_ident_entry ",")* import_ident_entry ","? ;
import_ident_entry = ident ("as" ident)? ;
```

```
# ワイルドカード（名前空間プレフィックスなし）
@use * from "core/tagaxes.hu"
@reference * from "entries/verbs.hu"

# ワイルドカード（名前空間プレフィックスあり）
@use * as stdlib from "stdlib/universal.hu"
@reference * as verbs from "entries/verbs.hu"

# 名前指定（項目ごとのエイリアスあり・なし混在可）
@use tense, aspect as a from "core/verbal_categories.hu"
@use (tense as t, aspect as a, number) from "core/verbal_categories.hu"
@reference faren as f, far from "entries/verbs.hu"
```

### `@use` の挙動

- 取り込み対象は宣言型（`tagaxis`, `@extend`, `inflection`）のみ
- `*` 指定時：`entry` が含まれていても**黙って無視**
- 名前指定時：`entry` を明示指定した場合は**コンパイルエラー**
- 循環インポートはコンパイルエラー

### `@reference` の挙動

- 取り込み対象は `entry` のみ
- `*` 指定時：宣言型が含まれていても**黙って無視**
- 名前指定時：宣言型を明示指定した場合は**コンパイルエラー**
- **循環参照を許容**：フェーズ1で全ファイルのエントリIDを収集した後、フェーズ2で個々の参照を解決する。ファイルロード順に依存しない

### 共通規則

- `*` の `as <ident>`：取り込んだシンボル全体への名前空間プレフィックス
- 名前指定の `as <ident>`：その項目のローカルエイリアス
- どちらもファイル先頭への配置を強制（hoistingの対象外）

---

## 文字列の型

DSLには二種類の文字列リテラルがある。型の不一致は**両方向でコンパイルエラー**。

| 型         | デリミタ    | 意味                                             |
| ---------- | ----------- | ------------------------------------------------ |
| `string`   | `"..."`     | 補間なしの自由テキスト                           |
| `template` | `` `...` `` | 幹・スロット名を `{name}` で参照できる補間文字列 |

```
# template（バッククォート）が出現できる文脈
inflection ルールの右辺:     [tense=present, person=1] -> `{pres}e`
slot の右辺:                 [tense=past] -> `{root}ta`
forms_override の右辺:       [participle=past] -> `gangen`
子音骨格テンプレート:          [tense=perfect] -> `C1aC2aC3a`

# string（ダブルクォート）が出現できる文脈
meaning, note, translation, proto, headword, display名等すべての自由テキスト
tokens 内のリンクなしリテラル: "." "Hwær"
```

templateの `{name}` はそのinflectionの `requires stems` で宣言されたキー、  
またはtagaxisの `@extend` で宣言された `slots` の名前のみ有効。それ以外はコンパイルエラー。  
ダブルクォート内の `{...}` はただの文字列として扱われ、チェックの対象外。

---

## Hoisting

同一ファイル内の以下の宣言型構文は定義順に関わらず前方参照可能：

| 構文         | Hoisting       |
| ------------ | -------------- |
| `tagaxis`    | ○              |
| `@extend`    | ○              |
| `inflection` | ○              |
| `entry`      | ×              |
| `@use`       | × （先頭強制） |
| `@reference` | × （先頭強制） |

---

## `tagaxis` — タグ軸定義

```
tagaxis parts_of_speech {
  role: classificatory
  display: { ja: "品詞", en: "Part of Speech" }
  index: exact
}

tagaxis tense {
  role: inflectional
  display: { ja: "時制", en: "Tense" }
}

tagaxis register {
  role: classificatory
  display: { ja: "語域" }
  # index省略 → インデックスなし
}
```

### `role` の種類

| role             | 意味                     | inflects_for への使用 | 逆引き自動生成 |
| ---------------- | ------------------------ | --------------------- | -------------- |
| `inflectional`   | 活用・曲用の次元         | ○                     | ○（自動）      |
| `classificatory` | 分類・検索用             | ×                     | ×              |
| `structural`     | 語根型など構造的メタ情報 | ×                     | ×              |

意味テキストは `meaning` フィールドとして `entry` が直接持つ。`tagaxis` の責務ではない。

### インデックス

| index値    | 意味                 |
| ---------- | -------------------- |
| `exact`    | 完全一致インデックス |
| `fulltext` | 全文検索インデックス |
| 省略       | インデックスなし     |

`role: inflectional` な軸は逆引きテーブルを自動生成（`index` 宣言不要）。  
`headword` と `meaning` は常にインデックス対象（宣言不要）。

---

## `@extend` — 宣言の拡張

既存の `tagaxis` に値を追加する名前付き拡張。`@use` によって可視になった場合のみ有効。

```ebnf
extend_decl = "@extend" ident "for" "tagaxis" ident "{" extend_body "}" ;
```

```
# tagaxis への値追加
@extend verb_noun_values for tagaxis parts_of_speech {
  verb { display: { ja: "動詞", en: "Verb" } }
  noun { display: { ja: "名詞", en: "Noun" } }
}

@extend stem_type_semitic for tagaxis stem_type {
  consonantal_3 {
    display: { ja: "三子音語根" }
    slots: [C1, C2, C3]      # structural軸の値はslotsメタ情報を持てる
  }
  consonantal_4 {
    slots: [C1, C2, C3, C4]
  }
}
```

```
# 利用側：@use で取り込んだ場合にのみ拡張が有効
@use verb_noun_values, stem_type_semitic from "extensions/pos.hu"
```

### スコープ規則

- `@extend` を `@use` したスコープ内でのみ拡張が有効（グローバルな副作用なし）
- `@use *` でインポートした場合は `@extend` 宣言も含まれる
- hoisting対象なので同一ファイル内の定義順は問わない

### 競合解決ルール

| ケース                                         | 挙動             |
| ---------------------------------------------- | ---------------- |
| 新しい値の追加                                 | マージ           |
| 既存の値の再定義                               | コンパイルエラー |
| 既存フィールドの上書き（`role`, `display` 等） | コンパイルエラー |
| 複数の `@extend` が同じ値を追加                | コンパイルエラー |

追加のみを許容し再定義を禁じることで、`@use` の順序に依存しない決定論的な動作を保証する。

### `slots` の厳密性

`slots` に宣言されたスロット名は `inflection` のtemplateルール内で使われる名前と照合される。  
宣言されていないスロット名がtemplateに出現した場合はコンパイルエラー。  
スロットに入る値の制約（「単子音1文字」など）はDSLの責務外で、コメントによる文書化で対応する。

---

## `inflection` — パラダイム定義

活用・曲用のパラダイムを定義する。形生成ロジックの本体。
エントリから `inflection_class` で参照されるか、`inflect` でインライン定義する。

```
inflection strong_I for {tense, person, number} {
  requires stems: pres, past

  [tense=present, person=1, number=sg] -> `{pres}e`
  [tense=present, person=2, number=sg] -> `{pres}est`
  [tense=present, person=3, number=sg] -> `{pres}eth`
  [tense=present, number=pl, _]        -> `{pres}en`
  [tense=past,    person=1, number=sg] -> `{past}`
  [tense=past,    person=2, number=sg] -> `{past}e`
  [tense=past,    number=pl, _]        -> `{past}en`
  [tense=future,  _]                   -> null    # この言語に未来形はない
}
```

### タグ条件リストの構文

```ebnf
tag_condition_list = "[" (tag_condition ",")* (tag_condition | "_") "]" ;
tag_condition      = ident "=" ident ;
```

`_` は末尾に1回だけ書けて「明示されていない軸は全て何でもよい」を意味する。  
末尾以外の位置に `_` が出現した場合はパースエラー。  
`_` なしのルールは全軸を明示した完全一致。

### ルールのマッチング優先順位

具体性は「明示されたタグ条件の数」で定義される：

```
[tense=present, person=1, number=sg]  # 具体性3 — 完全一致
[tense=present, number=pl, _]         # 具体性2 — personは何でもよい
[tense=future, _]                     # 具体性1 — person・numberは何でもよい
[_]                                   # 具体性0 — 全軸が何でもよい
```

1. 具体性が高いルールが勝つ
2. 同じ具体性のルールが複数マッチ → コンパイルエラー（曖昧さを実行時に持ち込まない）

### `for {}` の意味

パラダイムの展開次元（軸の直積がパラダイム空間を定義する）。  
`for {}` に宣言されていない軸がルール内に出現した場合はコンパイルエラー（展開不能）。

### `requires stems`

このinflectionをコンパイルするためにエントリが提供しなければならない幹。  
必要な幹が不足しているエントリに `inflection_class` が適用されるとコンパイルエラー。

```ebnf
stem_req       = ident ("[" tag_condition_list "]")? ;
requires_stems = "requires" "stems" ":" stem_req ("," stem_req)* ;
```

`stem_type` が `structural` な値を持つ場合（`@extend` で `slots` が宣言されている）、  
ident を付けて名前衝突を回避し、テンプレート内では `{ident.slot名}` で参照する：

```
inflection form_I for {tense, person, number} {
  requires stems: root1[stem_type=consonantal_3], aux

  [tense=perfect,   person=3, number=sg] -> `{root1.C1}a{root1.C2}a{root1.C3}a`
  [tense=imperfect, person=3, number=sg] -> `ya{root1.C1}{root1.C2}u{root1.C3}u`
  [tense=present,   _]                   -> `{aux}u`
  [_]                                    -> null
}
```

structural型でない通常の幹は従来通り `{stem名}` で参照する。

### `null` — 存在しない形の明示

`null` を明示することで「未定義（書き忘れ）」と「存在しない（意図的な欠損）」を区別する。  
`for {}` の全組み合わせが `null` を含めて網羅されていない場合はコンパイルエラー。  
`[_] -> null` で「他の全組み合わせは存在しない」をまとめて宣言できる。

### 膠着語的パラダイム（スロット合成）

```
inflection regular_verb for {tense, person_number} {
  requires stems: root

  compose root + tense_sfx + pn_sfx

  slot tense_sfx {
    [tense=present] -> ``
    [tense=past]    -> `ta`
    [tense=future]  -> `ru`
  }

  slot person_number {
    [person=1, number=sg] -> `m`
    [person=2, number=sg] -> `n`
    [person=3, number=sg] -> `s`
    [number=pl, _]        -> `ri`
  }

  override [tense=past, person=3, number=sg] -> `{root}tta`
}
```

### inflection への委譲

ルールの右辺に別の inflection を指定することで、条件に応じてパラダイムを委譲できる。  
条件分岐を inflection 内に閉じ込める。

```ebnf
rule_rhs = template_literal
         | "null"
         | ident "[" delegate_tag_list "]" ("with" "stems" "{" stem_mapping ("," stem_mapping)* "}")? ;

delegate_tag_list = (delegate_tag ",")* delegate_tag ;
delegate_tag      = ident "=" ident   (* 固定値: case=nominative *)
                  | ident             (* パススルー: case → 呼び出し元の値をそのまま渡す *)
                  ;

stem_mapping = ident ":" ident ;      (* 委譲先の幹名 : 呼び出し元の幹名 *)
```

```
# ラテン語形容詞：性によって異なる格変化に委譲
inflection adj_I_II for {case, number, gender} {
  requires stems: nom_m, nom_f, nom_n

  [gender=fem,  _] -> first_declension[case, number]            with stems { nom: nom_f }
  [gender=masc, _] -> second_declension[case, number]           with stems { nom: nom_m }
  [gender=neut, _] -> second_declension_neut[case, number]      with stems { nom: nom_n }
}

# 委譲先（独立したinflectionとして再利用可能）
inflection first_declension for {case, number} {
  requires stems: nom

  [case=nominative, number=sg] -> `{nom}a`
  [case=genitive,   number=sg] -> `{nom}ae`
  [case=nominative, number=pl] -> `{nom}ae`
  [_]                          -> null
}
```

委譲時のコンパイラ検証：

| 検証                                                      | 結果   |
| --------------------------------------------------------- | ------ |
| 委譲先 inflection が存在するか                            | エラー |
| `delegate_tag_list` が委譲先の `for {}` を網羅しているか  | エラー |
| `with stems` の写像が委譲先の `requires stems` を満たすか | エラー |
| パススルー軸が呼び出し元の `for {}` に存在するか          | エラー |

---

## 識別子の完全構文

```
(<namespace> ".")* <entry_id> ("#" <meaning_ident>)? ("[" <form_spec> "]")?
```

| 部分             | 意味                                         | 省略時                              |
| ---------------- | -------------------------------------------- | ----------------------------------- |
| `namespace.`     | `@use ... as` で付けた名前空間プレフィックス | 省略可                              |
| `entry_id`       | エントリの識別子（ユーザー定義）             | 省略不可                            |
| `#meaning_ident` | 多義語の意味指定                             | 意味が1つ、または意味を問わない場合 |
| `[form_spec]`    | 活用形の指定（タグ条件）                     | headword形（基本形）                |

### 使用例

```
# エントリ参照（語源・関連語など）
far

# 意味まで指定
far#motion

# 活用形指定（例文トークンなど）
faren[tense=present, person=1, number=sg]

# 意味と活用形を両方指定
faren#motion[tense=past, number=sg]

# 別名前空間
lat.aqua#liquid[case=nominative, number=sg]
```

`[form_spec]` が複数の形に一致する場合はコンパイルエラー（一意解決を強制）。

---

## `entry` — エントリ定義

### 活用の指定方法

エントリは `inflection_class` か `inflect`（インライン）のどちらか一方を持つ。

```ebnf
inflection_ref = "inflection_class" ":" ident
               | "inflect" "for" "{" axis_list "}" "{" rule_list "}" ;
```

`forms_override` は `inflection_class` がある場合のみ有効。`inflect` を使う場合は全ルールをインラインに記述するため `forms_override` は不要。

---

### 通常のエントリ（named class）

```
entry faren {
  headword: "faren"

  # 複数書記体系がある場合
  # headword {
  #   default: "食べる"
  #   kana:    "たべる"
  #   romaji:  "taberu"
  # }

  tags: [parts_of_speech=verb, register=formal]
  stems { pres: "far", past: "for" }
  inflection_class: strong_I

  meaning: "to go, to travel"

  # 多義の場合
  meanings {
    motion   { "to go, to travel (physically)" }
    progress { "to proceed, to advance (abstractly)" }
  }

  forms_override {
    [mood=subjunctive, tense=past] -> `fore`
    [participle=past]              -> `gangen`
    [tense=future, _]              -> null
  }

  etymology {
    proto: "*far-"
    cognates {
      far    "nominal root, same PIE origin"
      afaran "prefixed derivative"
    }
    derived_from: proto_far
    note: "Underwent i-umlaut in early period."
  }

  examples {
    example {
      tokens:      ic[] faren[tense=present, person=1, number=sg, mood=indicative] wille[] "."
      translation: "私は行く"
    }
  }
}
```

---

### インライン活用（irregular verb）

named class を作るほどでもない一品物の不規則語に使う。

```
entry sein {
  headword: "sein"
  tags: [parts_of_speech=verb]
  stems {}

  meaning: "to be"

  inflect for {tense, person, number} {
    [tense=present, person=1, number=sg] -> `bin`
    [tense=present, person=2, number=sg] -> `bist`
    [tense=present, person=3, number=sg] -> `ist`
    [tense=present, number=pl, _]        -> `sind`
    [tense=past,    person=1, number=sg] -> `war`
    [tense=past,    person=2, number=sg] -> `warst`
    [tense=past,    number=pl, _]        -> `waren`
    [_]                                  -> null
  }
}
```

---

### 分離動詞

`separability` を inflectional 軸として定義することで `tagaxis` の範囲内で対応できる。

```
entry aufmachen {
  headword: "aufmachen"
  tags: [parts_of_speech=verb]
  stems { base: "mach", prefix: "auf" }
  inflection_class: separable_verb

  meaning: "to open"

  examples {
    example {
      tokens: er[] aufmachen[tense=present, person=3, number=sg, separability=separated, part=verb]
              "die Tür"
              aufmachen[tense=present, person=3, number=sg, separability=separated, part=prefix] "."
      translation: "彼はドアを開ける"
    }
  }
}
```

`separable_verb` inflection は `separability` 軸と `part` 軸で分離形・非分離形を網羅する。  
forms テーブルには `part` 列が追加され、分離形の動詞部分・前綴り部分をそれぞれ格納する。  
`part` が null の場合は連続形（不連続形を持たない通常の語形）を意味する。

### `tokens` の構文

例文トークンの列。左から順に**表示上の語順**で並べる。

```
tokens: <token> (<token>)*

token:
  <ref>            # エントリ参照（form_specで活用形を指定）
  <string_literal> # リンクなしの生文字列（句読点・助詞・接辞など）
```

- `ref[]` （空のform_spec） → headword形（基本形）を使用
- 綴りが変更されても `tokens` の表示形は自動追従
- `form_spec` が一意解決できない場合はコンパイルエラー

---

## コンパイラの処理モデル

参照エラーの検出を遅延させるため、コンパイルは明確に二フェーズに分かれる。

```
フェーズ1：収集
  1. @use を再帰的に解決・ロード（循環はエラー）
  2. @reference のパスを収集してエントリファイルを列挙
     → 既出ファイルはスキップ（循環を許容）
  3. 全ファイルをパースしてASTを生成
  4. 宣言型（tagaxis, @extend, inflection）をホイスト
  5. 全 entry の ID をシンボルテーブルに登録（中身は未解決）

フェーズ2：解決
  全シンボルが揃った状態で以下を検証・解決
  - @reference の名前指定を解決（存在確認・型チェック）
  - 参照先 entry_id の存在確認
  - #meaning_ident の解決
  - [form_spec] の一意解決
  - directed リンクの DAG チェック
  - template 内 {name} の解決
  - 型不一致（string/template）の検証
  - パラダイム網羅性チェック
  - 非修飾名の曖昧性チェック
```

フェーズ1ではファイルのロードとID登録のみを行い、`@reference` の名前指定（個別エントリの指定）もフェーズ2で解決する。これにより、ファイルのロード順に依存せず、全シンボルが揃った状態で一貫した参照解決が可能になる。

名前空間の完全修飾名（`ns.entry_id`）もフェーズ1でシンボルテーブルに登録する。非修飾名が現在のスコープ内で複数のシンボルに解決可能な場合はコンパイルエラー（`as` エイリアスで回避）。

---

## コンパイラの検証項目

| 検証                                                       | フェーズ  | 結果   |
| ---------------------------------------------------------- | --------- | ------ |
| `_` がタグ条件リストの末尾以外に出現                       | パース時  | エラー |
| エントリIDの重複                                           | フェーズ1 | エラー |
| `@use` で entry を名前指定                                 | フェーズ1 | エラー |
| `@reference` で宣言型を名前指定                            | フェーズ1 | エラー |
| `@extend` で既存値を再定義                                 | フェーズ2 | エラー |
| 複数の `@extend` が同じ値を追加                            | フェーズ2 | エラー |
| 非修飾名が複数のシンボルに解決可能                         | フェーズ2 | エラー |
| `for {}` 未宣言軸がルールに出現                            | フェーズ2 | エラー |
| パラダイムの組み合わせ未網羅（`null`も含む）               | フェーズ2 | エラー |
| ルールの曖昧マッチ（同具体性）                             | フェーズ2 | エラー |
| エントリIDへの参照（存在確認）                             | フェーズ2 | エラー |
| `#meaning_ident` への参照                                  | フェーズ2 | エラー |
| `[form_spec]` の一意解決                                   | フェーズ2 | エラー |
| `requires stems` の不足                                    | フェーズ2 | エラー |
| `inflection_class` と `inflect` の両方を指定               | フェーズ2 | エラー |
| 委譲先 inflection が存在しない                             | フェーズ2 | エラー |
| 委譲の `delegate_tag_list` が委譲先 `for {}` を網羅しない  | フェーズ2 | エラー |
| 委譲の `with stems` が委譲先 `requires stems` を満たさない | フェーズ2 | エラー |
| 委譲のパススルー軸が呼び出し元 `for {}` に存在しない       | フェーズ2 | エラー |
| directed リンクの循環（DAGチェック）                       | フェーズ2 | エラー |
| templateが期待される文脈にstringが出現                     | フェーズ2 | エラー |
| stringが期待される文脈にtemplateが出現                     | フェーズ2 | エラー |
| template内の未宣言スロット名                               | フェーズ2 | エラー |

---

## コンパイラの生成物（SQLite）

| テーブル      | 内容                                                                                         |
| ------------- | -------------------------------------------------------------------------------------------- |
| `entries`     | id, entry_id, headword, meaning（FTS対象）                                                   |
| `forms`       | form_str, entry_id, tags, part（逆引き用。partはnullが連続形、非nullが不連続形の部分識別子） |
| `links`       | src_entry_id, dst_entry_id, link_type（語源・用例）                                          |
| `broken_refs` | 存在しないIDへの参照（整合性チェック結果）                                                   |

---

## 将来の拡張（現仕様対象外）

- **`phonrule`**：音韻規則の宣言的記述。現状は `forms_override` または inline `inflect` で対応。
- **Language Server Protocol対応**：エディタ上でのID補完・リンクナビゲーション・リアルタイムエラー表示。
- **リネームツール**：エントリIDの一括変更と参照の自動更新。
