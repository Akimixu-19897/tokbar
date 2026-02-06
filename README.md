# tokbar

macOS 状态栏（Tray）工具：统计并展示 Codex（cx）/Claude Code（cc）的 token 用量，可选 Today / Week / Month / Year，并在可获取模型价格时显示预估美元成本。

## 开发

- 安装依赖：`pnpm install`
- 启动开发：`pnpm tauri dev`
- 构建应用（debug）：`pnpm tauri build --debug --bundles app`

## 统计来源

- Codex：读取本机 `~/.codex/sessions/**.jsonl`（可用 `CODEX_HOME` 覆盖 `~/.codex`）
- Claude Code：读取 Claude 配置目录下的日志（跟随本机 Claude Code 的默认路径/环境变量）

## 模型价格与代理

模型价格默认从 GitHub RAW 获取：`model_prices_and_context_window.json`。在部分网络环境下可能无法直连。

- 若无法获取价格：状态栏/菜单会隐藏 `($xx.xx)`，菜单提示可点击打开 Proxy 设置。
- Proxy 设置会保存到：`~/.tokbar/proxy.json`
- 支持“聚合代理”（如 `127.0.0.1:7897` 或 `http://127.0.0.1:7897` / `socks5://...`），为空时再使用 HTTP/HTTPS/SOCKS5 分开配置。
