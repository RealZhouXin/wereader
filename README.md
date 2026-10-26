# rereader · Rust + Slint

一个 Kindle 原生应用起始项目，UI 使用 Slint，设备后端使用
[sverrejb/slint-kindle-backend](https://github.com/sverrejb/slint-kindle-backend)。
与同目录的微信读书安装包相互独立。

## 开发与预览

需要 Rust 1.92 或更新版本。

```sh
cargo run
# 无需图形环境，使用同一份 Slint UI 生成 600×800 灰度截图
cargo run -- --output preview.pgm
cargo test
```

默认 `desktop` feature 使用 winit 窗口后端和软件渲染。Kindle 构建关闭桌面依赖。
界面在 `ui/app.slint`，包括页码、上一页、下一页、退出按钮。Rust 回调在
`src/main.rs`；`build.rs` 编译并嵌入 Slint 资源；`src/software.rs` 仅用于无窗口截图。
Kindle 的显示、触摸和事件循环直接交给上游后端，不需要额外屏幕刷新脚本。

## 构建 Kindle 安装包

```sh
rustup target add armv7-unknown-linux-musleabihf
sh scripts/package.sh
```

生成 `dist/armv7-unknown-linux-musleabihf/install/`，将其内容合并复制到 Kindle USB
根目录。需要已越狱并能执行自定义程序的 Kindle；KUAL 可从菜单启动，书库脚本入口
需要 SH_Integration / Scriptlets。也可以通过设备 SSH 执行：

```sh
sh /mnt/us/rereader/start.sh
# 可选：自带字体、黑白模式或旋转
sh /mnt/us/rereader/start.sh --font /mnt/us/fonts/LXGWWenKai-Regular.ttf --rotation 90
sh /mnt/us/rereader/start.sh --black-and-white
```

默认嵌入 Tuffy 拉丁字体，可直接展示英文 Demo；中文界面需传入包含中文字形的字体。
没有配置定期休眠唤醒计划；该功能后续可通过后端的 `set_wake_schedule` 接入。

`.cargo/config.toml` 使用 Rust 随附的 `rust-lld`，目标为静态 musl，避免依赖 Kindle
系统 glibc。也提供软浮点目标：

`build.rs` 为 musl 生成空 `libdl.a` 兼容归档：Rust 自带 sysroot 未附带该归档，
而传递依赖仍传入 `-ldl`；实际 `dl*` 符号由 musl 的 `libc.a` 提供。

软浮点构建命令：

```sh
rustup target add arm-unknown-linux-musleabi
sh scripts/package.sh arm-unknown-linux-musleabi
```

目标须根据 CPU 和固件选择；静态链接不保证兼容所有旧内核、屏幕驱动和输入设备。
上游后端属于实验性项目，机型覆盖见其 README；本项目没有完成 Kindle 真机验收。

## 系统界面与恢复

启动器默认不暂停系统框架。若系统界面抢占屏幕，确认机型的进程后可从 SSH 使用：

```sh
KINDLE_MANAGE_FRAMEWORK=1 sh /mnt/us/rereader/start.sh
```

此选项会暂停 `awesome`、`cvm`、`KPPMainApp` 的现有进程，并在正常退出、应用报错
或收到 HUP/INT/TERM 时恢复本次暂停的 PID。它不能处理断电、内核崩溃、启动器被
SIGKILL 等情况；首次调试保留 SSH 通道，异常时重启设备。锁位于
`/tmp/rereader.lock`，重启后仍有锁时，先确认无运行实例再移除空锁目录。
日志位于 `/mnt/us/rereader/logs/`，按启动时间生成。

## 固定依赖与许可

固定 `slint = 1.18.1`、`slint-build = 1.18.1`、`slint-backend-kindle = 0.7.1`，
并附带 `Cargo.lock`。Kindle feature 的依赖只在 Linux 上启用。
本项目代码使用 MIT；Slint、后端和字体分别遵循各自许可，分发时保留相应声明。

## 当前验证

已在开发机通过 3 个单元测试、桌面构建的 Clippy 检查和 Slint 无窗口截图检查；
已生成 ARMv7 musl 静态 release 二进制与压缩安装包。尚未在 Kindle 真机测试，
软浮点目标目前仅提供构建配置。

参考：[后端 API 与设备说明](https://github.com/sverrejb/slint-kindle-backend)、
[Slint Rust 文档](https://docs.slint.dev/latest/docs/rust/slint/)。
