#!/usr/bin/env bash
set -euo pipefail

# 产品边界：原生 App 只能通过 Bundle 内 CLI 通信，不得回引 HTTP、浏览器或端口监听。
if rg -n -i 'axum|tokio|webview|tcplistener|hyper|warp|actix' \
  Cargo.toml src apps/macos/DiskIndexerApp; then
  echo '检测到被禁止的 Web/HTTP/监听依赖或代码。' >&2
  exit 1
fi

if rg -n 'localhost|127\.0\.0\.1|0\.0\.0\.0' src apps/macos/DiskIndexerApp; then
  echo '检测到被禁止的本地网络地址。' >&2
  exit 1
fi
