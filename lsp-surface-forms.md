# LSP: Surface Form Overlay & Display Mode

LSPサーバーに追加するカスタムリクエスト・通知の仕様。クライアント側の実装は別途行う。

## 背景

現在 `textDocument/inlayHint` でエントリ参照の解決形を `→ form` として表示している。これに加えて、エントリ参照式そのものを解決形で視覚的に置換する「overlay」モードを追加する。両モードの切り替えもサーバー側で管理する。

---

## 1. `hubullu/surfaceForms` リクエスト

エントリ参照の範囲とその解決形を返す。

**Method:** `hubullu/surfaceForms`
**Direction:** Client → Server

### パラメータ

```json
{
  "textDocument": { "uri": "file:///path/to/file.hu" }
}
```

### レスポンス

```json
{
  "items": [
    {
      "range": { "start": { "line": 5, "character": 4 }, "end": { "line": 5, "character": 38 } },
      "surfaceForm": "principiō",
      "tooltip": "principium[case=abl, number=sg]"
    }
  ]
}
```

| フィールド | 型 | 説明 |
|-----------|------|------|
| `range` | `Range` | 置換対象のソース式の範囲 |
| `surfaceForm` | `string` | 表示する解決形 |
| `tooltip` | `string?` | 元のソーステキスト。省略可 |

### Range の決め方

| パターン | range の始点 | range の終点 |
|---------|------------|------------|
| `entry_id[conditions]` | `entry_id` の先頭 | `]` の末尾 |
| `entry_id`（headword が異なる場合） | ident の先頭 | ident の末尾 |
| Glue chain: `ref₁ + ref₂` | `ref₁` の先頭 | `ref₂` の末尾 |
| Tilde chain: `ref₁ ~ lit` | `ref₁` の先頭 | 最後の要素の末尾 |

### Glue / Tilde chain の結合

チェーン全体で1つの `SurfaceFormItem` を返す。

- **Glue (`+`)**: 区切りなしで結合
- **Tilde (`~`)**: スペース区切りで結合
- リテラルトークンはそのまま含める

### 項目を返さない場合

既存の inlay hint と同じ条件:
- `entry_id` が解決できない
- 条件に一致する form がない
- bare `entry_id` で headword が entry name と同一

---

## 2. `hubullu/surfaceFormsRefresh` 通知

**Method:** `hubullu/surfaceFormsRefresh`
**Direction:** Server → Client
**Parameters:** なし

再解析後（通常は `didSave` 処理後）にクライアントへ送信する。`workspace/inlayHint/refresh` を送っている箇所と同じタイミングで送ればよい。

---

## 3. 表示モード管理

### モード定義

| モード | `textDocument/inlayHint` | `hubullu/surfaceForms` |
|--------|------------------------|----------------------|
| `"inlayHint"` (デフォルト) | 通常通り返す | 空配列を返す |
| `"overlay"` | 空配列を返す | 通常通り返す |
| `"off"` | 空配列を返す | 空配列を返す |

モードは `ServerState` にフィールドとして保持する（初期値 `"inlayHint"`）。

### `hubullu/setEntryRefDisplayMode` リクエスト

**Method:** `hubullu/setEntryRefDisplayMode`
**Direction:** Client → Server

```json
{ "mode": "overlay" }
```

**Response:** `null`

処理:
1. `ServerState` のモードを更新
2. `workspace/inlayHint/refresh` 通知を送信
3. `hubullu/surfaceFormsRefresh` 通知を送信
4. レスポンスを返す

### `hubullu/getEntryRefDisplayMode` リクエスト

**Method:** `hubullu/getEntryRefDisplayMode`
**Direction:** Client → Server
**Parameters:** なし

```json
{ "mode": "inlayHint" }
```

---

## 4. Capability 登録

`server_capabilities()` の `experimental` に追加:

```rust
"experimental": {
    "surfaceFormsProvider": true,
    "entryRefDisplayMode": true
}
```

---

## 5. 実装ガイド

### 変更対象ファイル

| ファイル | 変更内容 |
|---------|---------|
| `lsp/mod.rs` | リクエストハンドラの登録、`ServerState` にモードフィールド追加、capability 登録 |
| `lsp/inlay_hints.rs` | モードチェックの追加（既存関数の変更は不要、呼び出し側で分岐） |
| `lsp/surface_forms.rs` (新規) | surface form 生成ロジック |

### `lsp/mod.rs` の変更

**ServerState にフィールド追加:**

```rust
struct ServerState {
    // ... existing fields ...
    entry_ref_display_mode: EntryRefDisplayMode,
}

enum EntryRefDisplayMode {
    InlayHint,
    Overlay,
    Off,
}
```

**リクエストディスパッチに追加:**

```rust
"hubullu/surfaceForms" => handle_surface_forms(id, req, &state),
"hubullu/setEntryRefDisplayMode" => handle_set_display_mode(id, req, &mut state, &connection),
"hubullu/getEntryRefDisplayMode" => handle_get_display_mode(id, &state),
```

**`handle_inlay_hint` の変更:**

```rust
fn handle_inlay_hint(id: RequestId, req: Request, s: &ServerState) -> Response {
    if s.entry_ref_display_mode != EntryRefDisplayMode::InlayHint {
        return Response::new_ok(id, serde_json::to_value(Option::<Vec<InlayHint>>::None).unwrap());
    }
    // ... existing logic unchanged ...
}
```

**`handle_set_display_mode` の実装:**

モード更新後、`connection.sender` 経由で2つの通知を送る:
- `workspace/inlayHint/refresh`（標準LSP通知）
- `hubullu/surfaceFormsRefresh`（カスタム通知）

### `lsp/surface_forms.rs` の実装

`inlay_hints.rs` の解決ロジックを再利用する。主な違い:

- **inlay hint**: `span.end` の position だけ必要 → `InlayHint { position, label }`
- **surface form**: `span.start` と `span.end` の両方が必要 → `SurfaceFormItem { range, surfaceForm, tooltip }`

`.hu` ファイル: `EntryRef.span` は既に start/end を持っている。

`.hut` ファイル: トークン列から range を構築する。
- start = 最初のidentトークンの `span.start`
- end = `]` の `span.end`（条件あり）/ ident の `span.end`（条件なし）/ チェーン最後の要素の `span.end`

`inlay_hints.rs` の `find_resolved_entry`, `find_matching_form`, `parse_bracket_conditions`, `find_tilde_chain_end` はそのまま使えるか、共通モジュールに切り出す。

### JSON シリアライズ

レスポンス型を `serde::Serialize` で定義:

```rust
#[derive(Serialize)]
struct SurfaceFormsResult {
    items: Vec<SurfaceFormItem>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SurfaceFormItem {
    range: lsp_types::Range,
    surface_form: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tooltip: Option<String>,
}
```
