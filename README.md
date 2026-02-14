# chezmoi-tui

`chezmoi`の状態を視覚的に把握し、主要操作を安全に実行するためのRust製TUIです。

## 対応状況 (MVP)

- 3ペインUI
  - 左: 一覧 (`status` / `managed` / `unmanaged`)
  - 右上: diff/ファイル本文プレビュー
  - 右下: 実行ログ
  - `unmanaged` はディレクトリ展開対応（必要なファイルだけ選択可能）
- 操作メニュー
  - `apply`, `update`, `re-add`, `merge`, `merge-all`
  - `add`, `edit`, `forget`, `chattr`
  - `destroy`, `purge`
- 安全機構
  - 全操作で確認ダイアログ
  - `destroy`/`purge`は確認文字列入力を追加要求
- 設定保存
  - `~/.config/chezmoi-tui/config.toml` (XDG)

## 必要条件

- Rust 1.93+
- `chezmoi` がPATH上に存在
- macOS / Linux

## 実行

```bash
cargo run
```

## キーバインド

- `1` / `2` / `3`: 一覧切替 (`status`, `managed`, `unmanaged`)
- `j` / `k` or `↑` / `↓`: 選択移動
- `l` / `→`: ディレクトリ展開 (`unmanaged`ビュー)
- `h` / `←`: ディレクトリ折りたたみ (`unmanaged`ビュー)
- `Tab`: ペインフォーカス移動
- `Enter` or `d`: 選択対象のdiff取得
- `v`: 選択対象のファイル本文プレビュー（read-only）
  - 拡張子ベースでシンタックスハイライト表示
- `unmanaged`ビューではファイル選択時に自動プレビュー
- `j` / `k`: `Detail`フォーカス時はプレビュー/差分をスクロール
- `PgUp` / `PgDn`, `Ctrl+u` / `Ctrl+d`: `Detail`フォーカス時に大きくスクロール
- `a`: アクションメニュー
- `e`: `edit`確認ダイアログ
- `r`: 一覧更新
- `q` or `Ctrl+C`: 終了

## 実装メモ

- `managed --format json`が環境によってプレーンテキスト出力になるケースを考慮し、JSON/行パースの両対応を実装しています。
- `status`は2カラム記号を内部モデルへ変換して表示します。
- `add`でディレクトリを直接選択した場合は事故防止のため実行せず、展開して個別ファイルを選ぶ運用にしています。

## テスト

```bash
cargo test
```
