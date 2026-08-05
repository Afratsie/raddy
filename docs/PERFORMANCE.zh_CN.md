# 性能基线

> 固定脚本 + 固定环境跑出的 **可复现** QPS / P99 记录。数字本身不是目标，**可复现**才是——换个机器/配置重跑同一脚本即可对比。

## 方法与工具

- **工具**：仓库内 `examples/loadtest.rs`（`cargo run --release --example loadtest -- <url> <并发> <请求数>`）。
- **负载形态**：每个请求**新建一条 TCP 连接**（`Connection: close`），客户端 → raddy → 上游各一条。这是"连接建立开销"主导的形态，**低于** keep-alive 稳态吞吐；基线可复现，但不代表稳态上限。
- **拓扑**：`raddy run`（release 构建）代理到本地 `python3 http.server` 上游（小响应体 `hello world`），明文 HTTP。
- **指标**：QPS（总请求/总耗时）、p50 / p99（单请求往返延迟，含 TCP 建连）。

## 测试环境

| 项 | 值 |
|---|---|
| CPU | Intel Core i5-11320H @ 3.20GHz（4 核 8 线程） |
| 内核 | Linux 7.1.5-arch1-2 |
| raddy | release 构建（`cargo build --release`） |

## 结果（2026-08-05）

| 并发 | 请求数 | QPS | p50 | p99 |
|---|---|---|---|---|
| 8 | 10,000 | 7,088 | 0.54ms | 2.57ms |
| 16 | 20,000 | 5,651 | 0.52ms | 3.42ms |
| 32 | 20,000 | 4,380 | 0.59ms | 3.63ms |

> 说明：QPS 随并发回落，符合"每请求新连接"形态——高并发下客户端侧连接建立/系统资源成为瓶颈，而非 raddy 本身。p99 稳定在 2.5–3.6ms。

## 复现

```bash
cargo build --release
cargo build --release --example loadtest

# 起上游 + raddy（替换端口）
python3 -m http.server 19200 --bind 127.0.0.1
./target/release/raddy run -c <代理到 127.0.0.1:19200 的 Raddyfile>

# 压测
./target/release/examples/loadtest http://127.0.0.1:8098/ 16 20000
```

## 后续

- 引入 keep-alive 负载形态（连接复用）以接近稳态吞吐。
- 若引入 CI 性能回归，以**相对裸 Pingora 的代理层开销**为阈值，而非绝对值。
