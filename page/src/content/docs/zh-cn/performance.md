---
title: 性能
description: 由固定压测脚本产出的可复现 QPS / P99 基线。
---

> 一份由固定脚本、固定机器产出的**可复现** QPS / P99 记录。数字不是目标 ——
> 可复现性才是。在任何机器上重跑同一脚本即可对比。

## 方法与工具

- **工具**:仓库内的 `examples/loadtest.rs`
  (`cargo run --release --example loadtest -- <url> <concurrency> <requests>`)。
- **负载形态**:每个请求都从客户端 → raddy → 上游**新建一条 TCP 连接**
  (`Connection: close`)。该形态以连接建立开销为主,因此会**低估** keep-alive
  稳态吞吐;它是可复现的,而非上限。
- **拓扑**:`raddy run`(release 构建)代理到本地 `python3 http.server` 上游
  (小的 `hello world` 响应体),纯 HTTP。
- **指标**:QPS(总请求数 / 墙钟时间),p50 / p99(每次请求往返,含 TCP
  连接)。

## 测试环境

| 项目 | 值 |
|---|---|
| CPU | Intel Core i5-11320H @ 3.20GHz(4 核 / 8 线程) |
| 内核 | Linux 7.1.5-arch1-2 |
| raddy | release 构建(`cargo build --release`) |

## 结果(2026-08-05)

| 并发 | 请求数 | QPS | p50 | p99 |
|---|---|---|---|---|
| 8 | 10,000 | 7,088 | 0.54ms | 2.57ms |
| 16 | 20,000 | 5,651 | 0.52ms | 3.42ms |
| 32 | 20,000 | 4,380 | 0.59ms | 3.63ms |

> QPS 随并发上升而下降 —— 对每请求一连接形态来说符合预期:瓶颈在客户端侧
> 的连接建立,而非 raddy 本身。p99 保持在 2.5–3.6ms 区间。

## 复现

```bash
cargo build --release
cargo build --release --example loadtest

# 启动上游与 raddy(按需替换端口)
python3 -m http.server 19200 --bind 127.0.0.1
./target/release/raddy run -c <代理到 127.0.0.1:19200 的 Raddyfile>

# 压测
./target/release/examples/loadtest http://127.0.0.1:8098/ 16 20000
```

## 未来工作

- 增加 keep-alive 负载形态(连接复用)以逼近稳态吞吐。
- 若引入 CI 性能回归门,请以**相对裸 Pingora 的代理开销**作为阈值,而非
  绝对值。
