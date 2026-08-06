---
title: CLI 参考
description: raddy 的每个子命令及其选项。
---

`raddy` 二进制有三个主要子命令,外加一个迁移辅助命令。

## `raddy run`

前台运行反向代理服务器。

| 选项 | 默认值 | 说明 |
|---|---|---|
| `-c, --config <file>` | `Raddyfile` | Raddyfile 路径 |
| `--cert-dir <dir>` | `raddy_certs` | ACME 证书与账户凭据目录 |
| `--acme-directory <url>` | Let's Encrypt 生产环境 | ACME 目录 URL |
| `--acme-root-pem <file>` | — | 信任 ACME 服务器的 PEM 根 CA(测试服务器如 Pebble 必需) |
| `--access-log <file>` | — | 将结构化 JSON 访问日志追加到此文件 |
| `--metrics-addr <addr>` | — | 在此地址暴露 Prometheus `/metrics`(例如 `127.0.0.1:9100`) |
| `--pidfile <file>` | — | 将本进程 PID 写入此文件,供 `raddy upgrade` 定位 |
| `--upgrade-sock <sock>` | `/tmp/raddy_upgrade.sock` | 升级期间移交监听 fd 的 Unix 套接字 |
| `-u, --upgrade` | — | 以零停机升级的*新*一侧启动(通常由 `raddy upgrade` 派生) |
| `-t, --test` | — | 校验配置与构造后退出 0/1,不绑定任何监听器(`raddy upgrade` 的预检) |

## `raddy upgrade`

零停机二进制升级(需要 `--pidfile`):预检新二进制,以 `-u` 派生替代进程,然后
向运行中实例发送 SIGQUIT。与 `raddy run` 共享相同的选项。

## `raddy check`

校验 Raddyfile 并退出——与**重载执行的校验完全相同**。通过 `check` 的配置能
干净重载,反之亦然。

```bash
raddy check -c Raddyfile   # 输出 "Raddyfile: ok",退出 0;或输出错误并退出 1
```

## `raddy import`

将 Caddyfile 或 nginx.conf 子集转换为 Raddyfile。**独立转换器**:绝不改动
Raddyfile 文法,并在打印前(通过重载所用的同一管道)校验自身输出。

```bash
raddy import caddyfile <source> [-o <output>]
raddy import nginx    <source> [-o <output>]
```

省略 `-o` 则把 Raddyfile 打印到 stdout。

## 退出行为

`check` 对合法配置退出 0,否则退出 1。`run` 与 `upgrade` 在启动错误时退出 1
(例如非法配置,因此非法配置绝不会启动进程)。`import` 在无可转换内容或生成的
Raddyfile 校验失败时退出 1。
