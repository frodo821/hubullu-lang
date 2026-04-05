# Postprocessor（音素変換・音訳パイプライン）

> **ステータス**: 設計段階（未実装）

Postprocessorは、エントリの形（form）に対して音素変換・経時的音韻変化シミュレーション・文字体系間変換を行う宣言型パイプラインである。

## 設計目標

- プログラマブルなAPIとして提供し、renderで音訳を注釈として挿入
- 経時的な音韻変化のシミュレーション（例：古英語→中英語）
- 文字体系間の変換（例：ラテン文字→カタカナ、アラビア文字）
- エントリへの格納は行わず、API / render時に動的に計算

## 基本構造

```hu
@postprocessor my_lang {
  # 素性軸の定義
  axis manner     = [stop, fricative, nasal, approximant, trill, affricate]
  axis articulator = [bilabial, labiodental, alveolar, postalveolar, velar, glottal]
  axis voicing    = [voiceless, voiced]
  axis type       = [consonant, vowel]
  axis height     = [close, near_close, mid, open_mid, open]
  axis backness   = [front, central, back]
  axis rounding   = [rounded, unrounded]

  # 音素定義
  phoneme p [type:consonant, manner:stop, articulator:bilabial, voicing:voiceless]
  phoneme b [type:consonant, manner:stop, articulator:bilabial, voicing:voiced]
  phoneme f [type:consonant, manner:fricative, articulator:labiodental, voicing:voiceless]
  phoneme a [type:vowel, height:open, backness:front, rounding:unrounded]

  # 正書法→音素のマッピング（多文字対応）
  phoneme ʃ [type:consonant, manner:fricative, articulator:postalveolar, voicing:voiceless]
    spelling: "sh"
  phoneme tʃ [type:consonant, manner:affricate, articulator:postalveolar, voicing:voiceless]
    spelling: "ch", "tch"

  # 略記（alias）
  alias V = [type:vowel]
  alias C = [type:consonant]
  alias T = [manner:stop, voicing:voiceless]

  # 変換ルールセット
  ruleset grimm ordered {
    p > f
    t > θ
    k > x
  }

  ruleset palatalize simultaneous {
    [manner:stop, articulator:velar] > [articulator:postalveolar, manner:affricate]
      / _[type:vowel, backness:front]
  }

  # パイプライン関数
  fn romanize(s) = s | grimm
  fn to_kana(s)  = s | to_kana_rules
}
```

## 素性体系

### 軸定義（`axis`）

素性の次元を定義する。各軸は排他的な値の集合を持つ。

```hu
axis manner = [stop, fricative, nasal, approximant, trill, affricate]
```

### 音素定義（`phoneme`）

各音素を素性バンドルとして定義する。素性は `axis:value` 形式で指定。

```hu
phoneme p [type:consonant, manner:stop, articulator:bilabial, voicing:voiceless]
```

`spelling:` を指定すると、正書法→音素列の自動変換ルールが生成される。
spellingがない音素は文字と音素が同一とみなされる。

```hu
phoneme ʃ [type:consonant, manner:fricative, articulator:postalveolar, voicing:voiceless]
  spelling: "sh"
```

### 素性バンドルの参照

ルール中では常に `axis:value` 形式で素性を参照する。
値の名前が複数の軸に存在しうるため、軸名は省略不可。

```hu
# 条件側も変換側も軸を明示
[manner:stop, voicing:voiceless] > [manner:fricative] / [type:vowel]_[type:vowel]
```

変換側（`>`の右辺）で指定した軸の値のみが変更され、
指定していない軸の値は入力音素から保持される。

```hu
# articulator軸だけ変更、他は保持
[articulator:bilabial] > [articulator:velar] / _[backness:back]
# p → k, b → g, m → ŋ
```

### 略記（`alias`）

頻出する素性バンドルに名前を付ける。展開されるだけで新しい意味論は持たない。

```hu
alias V = [type:vowel]
alias C = [type:consonant]
alias T = [manner:stop, voicing:voiceless]

# ルール内で使用
T > [manner:fricative] / V_V
```

## 変換ルール

### ルールセット（`ruleset`）

```hu
ruleset name ordered|simultaneous {
  rule1
  rule2
  ...
}
```

- **`ordered`**: 上から順に適用。feeding/bleeding関係が有効（音韻変化シミュレーション向き）
- **`simultaneous`**: 全ルールを一括適用。ある規則の出力が別の規則の入力にならない（文字体系変換向き）

### ルール構文

```
入力 > 出力
入力 > 出力 / 左文脈 _ 右文脈
```

入力/出力は個別音素または素性バンドル。

```hu
# 個別音素
p > f

# 素性ベース
[manner:stop, voicing:voiceless] > [manner:fricative]

# 文脈依存
[manner:stop] > [voicing:voiced] / [manner:nasal]_

# 形態素境界（~）を文脈に使用
n > m / _[~][articulator:bilabial]
```

### 形態素境界

以下の2つを統一的に形態素境界として扱う：

- inflectionのcomposeで挿入される内部境界（`\0`）
- `.hut`形式の結合記号（`~`）

ルール中では `~` で参照可能。最終出力時にstripされる。

## 正書法→音素列変換

`spelling:` 定義に基づく正書法→音素列変換は暗黙的に行われる。
rulesetに入力される時点で文字列は音素列に変換済みとなる。

変換は最長一致で適用される。

### 優先順位

1. entryに `pronunciation` が明示されていればそれを使用
2. なければ `spelling:` ルールで自動変換

```hu
entry knight {
  headword: "knight"
  pronunciation: /naɪt/
  meaning: "騎士"
}
```

inflectionルールの右辺でも発音を指定可能：

```hu
[tense=past, _] -> `went` /wɛnt/
```

## パイプライン関数（`fn`）

rulesetをパイプ（`|`）で連鎖させる。render/APIから名前で呼び出し可能。

```hu
fn romanize(s) = s | grimm | to_latin
fn kana(s)     = s | palatalize | to_kana
```

## Namespace

`@postprocessor name { ... }` のブロック名がnamespaceとなる。

```hu
@postprocessor my_lang {
  axis tone = [high, mid, low]
  # tone は my_lang.tone — 自分のnamespace内なので修飾不要
}
```

### 外部postprocessorの参照

他のpostprocessor（特に組み込みIPA）の定義を参照するときは
`namespace.axis:value` 形式を使用する。

```hu
@use "std:ipa"

@postprocessor my_lang {
  # 明示的にnamespace付き
  phoneme sh [ipa.type:consonant, ipa.manner:fricative,
              ipa.articulator:postalveolar, ipa.voicing:voiceless]
    spelling: "sh"
}
```

### Namespace省略（`@use ns.*`）

postprocessorブロック内で `@use ns.*` と書くと、
そのブロック内で `ns.` プレフィックスを省略できる。

```hu
@postprocessor my_lang {
  @use ipa.*

  # ipa. が省略可能
  phoneme sh [type:consonant, manner:fricative,
              articulator:postalveolar, voicing:voiceless]
    spelling: "sh"

  # 自分のnamespaceの独自軸はそのまま
  axis tone = [high, mid, low]
  phoneme á [type:vowel, height:open, backness:front, tone:high]
}
```

衝突時は明示が必須。曖昧な場合はコンパイルエラーとなる。

## 組み込みIPAモジュール

`std:ipa` として提供。IPAの標準的な素性体系（調音点・調音法・有声性など）と
全IPA音素の定義を含む。

```hu
@use "std:ipa"

@postprocessor my_lang {
  @use ipa.*
  # IPAの全axis/phonemeが使える
}
```

組み込みモジュールは `@use "std:name"` でインポートする
（`std:` スキームについては `06-imports.md` を参照）。

## 利用場面

### Renderでの音訳注釈

postprocessorの `fn` をrender時に呼び出し、
ルビや括弧書きで音訳を付与する。

### 経時的音韻変化シミュレーション

ordered rulesetを連鎖させて、歴史的音韻変化を再現する。

```hu
@postprocessor old_english {
  @use ipa.*
  phoneme æ [type:vowel, height:open, backness:front, rounding:unrounded]
    spelling: "ae"

  ruleset i_mutation ordered {
    [backness:back] > [backness:front] / _C*[height:close, backness:front]
  }

  fn to_ipa(s) = s | i_mutation
}

@postprocessor middle_english {
  @use ipa.*

  ruleset open_syllable_lengthening ordered {
    [type:vowel, length:short] > [length:long] / _C[type:vowel]
  }

  fn from_oe(s) = s | open_syllable_lengthening
}
```

### 文字体系変換

simultaneous rulesetで文字体系を変換する。

```hu
@postprocessor transliteration {
  @use ipa.*

  ruleset to_kana simultaneous {
    ka > カ
    ki > キ
    ku > ク
    ke > ケ
    ko > コ
    # ...
  }

  fn kana(s) = s | to_kana
}
```
