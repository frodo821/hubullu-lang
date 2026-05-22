# Proposal: スロットベース形態論モデル（lazy slot morphology）

hubullu の形態論モデルを **eager なパラダイム生成** から **lazy なスロット解析・検査** へ転換し、スロット構造を再帰的にする提案。compile の組み合わせ爆発を解消しつつ整合性検査を保持し、形態素を first-class なエントリにして文法化のモデル化を可能にする。

a priori 祖語プロジェクト（`/Users/csakai/repos/writing/a_priori/run_01/`）での議論が起点。

## 1. 動機

現状の `inflection` は **eager**：エントリごとにパラダイム全体（全軸の直積）を compile 時に生成する。a priori proto の動詞は 10 軸（question, negation, mood, voice, tense, aspect, abs_number, abs_person, erg_number, erg_person）で数万形/エントリーになり、`hubullu compile` は数分・1〜2 GB を要する。

加えて現状では:
- **形態素が first-class でない** — `-əp` PAST 等は `slot` 内のリテラル文字列として埋め込まれ、複数 slot/inflection で重複する（`-əd` は動詞 abs_num・erg_num・名詞 num で同一形態素なのに 3 箇所に文字列重複）
- 文法化（接語 ⇄ 接辞）のような **形態論の通時変化**を表現する足場がない

目標:
1. compile の組み合わせ爆発を消す
2. 整合性検査（形態素配列の well-formedness）は保持する
3. 形態素を first-class なエントリにする（DRY 共有・文法化のモデル化）

## 2. コア転換：eager 生成 → lazy 解析・検査

- **compile** = スロット構造の検査のみ（`O(slots)`、直積生成なし）
- **render** = 与えられた形態素構造をスロット文法に照らして充填・検証する
- **0 parse = error**（スロット構造に充填不能 = 形態素配列違反）
- **≥1 parse = well-formed** — parse 数は数えない。複数 parse（曖昧）は render にとって非問題（§6）

これは生成モデルから認識/解析モデルへの転換であり、enumerate しないので compile は安価、検証は実際に書かれた形にだけ遅延して走る。

## 3. スロット構造

### 3.1 typed slot + 素性付き形態素

- **スロット**は「どの素性軸を受けるか」を宣言する
- **形態素エントリ**は「どの素性を担うか」を宣言する
- スロット充填は本質的に**形態素 parsing**（順序付き文法に対する照合）

同音だが異スロットの形態素（proto の `-əs` は ABS.3 でも ERG.3）はスロット**順序**で解決される。型付き形態素だけでは足りず、スロット構造は順序付き文法でなければならない。

### 3.2 eager / lazy スロット

スロットは `eager` か `lazy` に分かれ、レイアウトは **`[lazy*][eager*][lazy*]`** に制約される（eager 核は連続する 1 ブロック）。

- **eager 核** = 構造化されたパラダイム部。厳密な順序、軸組織化。望めば（小さい）直積として列挙可能
- **lazy 周辺** = 自由膠着ゾーン。無型の catch-all、permissive

**catch-all warning**: lazy の catch-all は permissive だが、「定義済みスロットの素性を担う形態素」が catch-all に落ちた場合は **warning** を出す。これは「構造化されるべきだったのに、その線形位置でスロット文法が届かなかった」= 形態素配列違反を catch-all が黙って救済してしまうのを un-silence する役。真に周辺的な素材（無素性の clitic/粒子、どのスロットも扱わない軸の素性をもつ語）は黙って catch-all で良い。

例: `katab ~ əs ~ əs ~ əs` — abs_pers・erg_pers が先頭 2 つで埋まり、3 つ目の `əs` が catch-all へ。`əs` は person を担い person は slot-defined → warning（over-supply が captured される）。

### 3.3 variadic slot

「任意個」のスロットを定義できる必要がある。lazy 周辺（自由膠着ゾーン）に必須。

### 3.4 再帰

**lazy も eager も再帰的に node を持てる** — スロット充填子は再帰的に `[lazy*][eager*][lazy*]` 構造そのものになりうる。結果、形態論構造はフラットなスロット列ではなく **再帰木**になる。

この再帰木は **文法化状態の表現**になる:
- **独立語** = top-level、自前のフル `[lazy*][eager*][lazy*]`
- **接語** = host の lazy 娘、ただし**自前の内部構造（パラダイム）を保持**
- **接辞** = host の eager 核に潰れ込み、独立した構造を失う

文法化 = 「ユニットが木の内側へ移動し、自前の構造を失い host の eager 核へ併合される過程」。再帰モデルはこの cline を表現可能にする。通時 conlanging を支える hubullu にとって重要な性質。

詰める細部: (1) 再帰の終端 — leaf の真の catch-all をどう定義するか、(2) catch-all warning が再帰構造のどのレベルで効くか。

### 3.5 不連続性

形態論の「不連続」には 2 種類あり、別機構で扱う:

- **circumfix**（1 形態素が stem を包んで不連続）= **wrapping node**。circumfix を外側の node とし、その 2 部分を node の前後の lazy、stem を node の eager スロットに入れる。「circumfix が stem を包む」。これは eager スロットも再帰 node を持てることを要求するが、派生語幹（動詞の名詞化など）でも eager 核の中身は複雑 node になるので元々必要。
  - flatter な代替: circumfix を「2 スロットを連動して埋める 1 エントリ」にする（eager 再帰不要、代わりに「1 エントリが複数スロットを束ねる」機構を足す）
- **不連続テンプレート**（語幹そのものの内部不連続、root-and-pattern）= **stem 構造の機構**。hubullu は既にこれを持つ（`stems { root: "ktb" }` + `root_type` の `slots: [C1,C2,C3]` + `{root.C1}a{root.C2}…` の補間）。「stem 構造に入れられるものを拡張する」のが正しいレバーで、slot モデルを触る話ではない。stem 構造が**内部スロット位置**を宣言できるよう拡張すれば、同じレバーで **infix** まで届く。

**Phase 6 で採用した実装**（2026-05-16、reshape 後のスコープ縮小版）:

- **circumfix**: flatter な代替「1 エントリが 2 スロット位置に束縛」を採用（wrapping node + eager 再帰は不要）。形態素エントリの `headword` 内に splice marker `^` を埋め込み（例: `"ge^t"`）、`slot NAME circumfix matching [...]` で宣言したスロットを `compose` chain 内で**ちょうど 2 回**参照する。レンダー時、`^` で前半・後半に split し、第 1 出現に prefix、第 2 出現に suffix を流し込む。`werken[tense=past_ptcp]` → `gewerkt`。Phase 2 で chain 出現回数を静的検査（2 回未満／超過は compile error）。
- **infix**: 既存の stem-template 機構を拡張。`@extend` 値に `infix_positions: [after_C1, after_C2]` を追加し、`build_struct_stems` がそれらを空文字列で pre-populate。同名の `slot NAME infix matching [...]` を inflection に宣言する（**chain には現れない** — Phase 2 で検査）。レンダー時、infix slot の filler の surface を `struct_stems[stem_name][slot_name]` に splice in し、eager rule の template `{root.after_C1}` が解決できるようにする。`kataba[vowel_pattern=perfect_a_a]` (root=`ktb`) → `katab`。当初 proposal の `Nested` 機構は不要（compose chain がフラットなので nested grammar が消えた reshape の副産物）。

## 4. 構文

スロット構造をもつエントリ参照の構文:

```
entry[axes][slots]
```

- **`[axes]`** = eager 核のパラダイム素性参照（既存の `kataba[tense=past, …]` 構文）
- **`[slots]`** = 明示的スロット充填。各スロットにエントリ参照、または `{…}`（順序付きリスト、variadic slot 用）を割り当てる
- **`[][slots]` 必須** — 第 1 ブラケットは位置固定。axes を省略する場合も空 `[]` を書く（`entry[X]` が axes か slots か曖昧にならないように）
- **`<…>` は不採用** — hubullu の `.hut` は既に XML ライクタグ（`<tag>`）を持つので衝突する
- スロット値の `{…}` は **set ではなく順序付きリスト**（形態素順は有意）。記法は `{}`（`[]`/`<>` は取られている）だが意味はリストと明記する
- ネスト深度に上限なし（深くネストする自然言語は実在せず、縛っても表現力を削るだけ）

充填子が再帰的に `entry[axes][slots]` でありうるため、§3.4 の再帰木がそのまま構文になる。

## 5. `~` との関係 — 別レジスタ共存

`[axes][slots]` と F1b の `~` は「2 つの記法」ではなく「2 つの**現象**」を表す:

- **`[axes][slots]`** = パラダイム的屈折形態論（構造木）
- **`~`** = 非パラダイム的膠着 — 派生・複合・語彙融合（herb~arium の `-arium` は "herb" のパラダイムのセルではなく語形成）

冗長ではないので共存して当然。さらに**合成可能**: `~`-融合語が `[slots]` の stem になれるし、`[slots]`-語が `~`-複合に入れる。`~` は `.hut` 散文（例文）向きの軽量・語らしい記法で、F1b の phonrule 境界の役も保つ（派生・複合の境界依存音変化にちょうどいい）。

## 6. 曖昧性の扱い

- **同音形態素**（proto の `-əs` = ABS.3 = ERG.3）: 区別が重要なら**別エントリ**を立てればよい。さらに `.hut` が形態素を**エントリ参照で**書く（`katab ~ abs3_sfx ~ erg1_sfx`）なら、前向き（エントリ→表層）の render に表層同音は一切影響しない
- **複数 parse**: 同じ構造の複数 parse はすべて同一の表層形を生む（形態素列は不変、変わるのはラベル付けだけ）。render は文字列を出すので曖昧は非関心。「>1 で warning」は不要。曖昧さが再浮上するのは**分析（gloss）が要るとき**だけで、それは下流ツール（[[hu-md-proposal]] の `<hu-gloss>` 等）の関心事

## 7. phonrule とのシナジー

- `[slots]` は構造が明示なので render は**全境界を知っている** → 正確な `\0`（あるいは名前付き境界）を打てる。`~` 列を parse して境界を復元するより精密
- gloss が自明になる（書いた構造がそのまま形態素分析）

## 8. 移行

- eager モデルを**置換**するか**共存**（inflection ごとに eager/lazy 宣言）か — hubullu には既存ユーザー・`examples/` があるので無視できない
- a priori proto は現状 eager パラダイム。lazy モデルへの移行は段階的に行う
- 既存の `inflection` 構文との後方互換をどう設計するかは要検討

## 9. 理論的位置づけ

- eager/lazy の選択自体は**出力（フォーム）に不変** = 純粋に工学
- だが `[lazy*][eager*][lazy*]` の制約と再帰構造は「**階層的・再帰的な形態論**」という light theory をベイクインしている。主流の見方なので安いコミットだが、ゼロではない
- 通時プロジェクトでは eager/lazy 境界の位置と、それが proto→daughter で「動くこと」自体が言語的主張になる（接語→接辞の文法化＝ユニットが再帰木を降りる）。単一辞書のスナップショットでは bookkeeping だが、プロジェクトの時間深度をまたぐと理論を担う — そしてこのプロジェクトはまさに形態論変化を扱うので、この light theory はむしろ適切

## 10. 解決すべき設計判断

1. **再帰の終端定義** — leaf の真の catch-all をどう規定するか
2. **catch-all warning のレベル** — 再帰構造のどのレベルで効くか。warning 抑制（意図的な同音周辺 clitic）の手段
3. **eager 核の列挙ポリシー** — eager 核を実際に直積展開するか、それも lazy にするか
4. **移行戦略** — eager `inflection` との共存方法、後方互換
5. **circumfix の実装** — wrapping node（eager 再帰）か multi-slot bound entry か
6. **stem 構造の拡張範囲** — infix（内部スロット位置）までカバーするか
7. **slot / 形態素の型宣言構文** — スロットが受ける軸、形態素が担う素性をどう書くか

## Critical Files for Implementation

- `/Users/csakai/repos/anl-tools/hubullu/src/ast.rs` — slot 構造・再帰 node・`entry[axes][slots]` の AST
- `/Users/csakai/repos/anl-tools/hubullu/src/parser.rs` — `entry[axes][slots]` 構文、`{…}` リスト、slot/形態素の型宣言
- `/Users/csakai/repos/anl-tools/hubullu/src/inflection_eval.rs` — lazy parse・スロット充填・catch-all warning
- `/Users/csakai/repos/anl-tools/hubullu/src/phase2.rs` — スロット構造の compile 時検査
- `/Users/csakai/repos/anl-tools/hubullu/src/phase1.rs` — 形態素エントリの登録
