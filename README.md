# rereader · Rust + Slint

一个支持微信读书扫码登录与书架展示的 Kindle 原生应用，UI 使用 Slint，设备后端使用
[sverrejb/slint-kindle-backend](https://github.com/sverrejb/slint-kindle-backend)。
与同目录的微信读书安装包相互独立。

微信读书接入采用墨水屏客户端路线，参考 `weread-omni`：

- [接口清单与接入安排](docs/weread-api.md)：登录、书架、目录、笔记及正文/进度待验证接口。
- [源码研究与移植安排](docs/weread-eink-research.md)：设备配置、会话契约、传输行为和 Rust 实施顺序。
- [wxBook 墨水屏实现研究](docs/wxbook-interface-research.md)：正文、进度及缓存实现参考；其 HTTP 层采用独立的 Web 会话协议。

## 扫码登录与书架

运行 `cargo run`，点击 **登录 / 换码**，使用微信扫描二维码并在手机上确认。登录成功后加载电子书书架，以每页 4 列 × 3 行展示封面和书名；底部箭头翻页，**刷新** 更新书架，**退出登录** 删除本地登录凭据。顶部支持按书名或作者搜索，读完的书显示标记。当前只展示电子书列表，不提供打开正文或有声书入口。

**登录 / 换码** 会取消旧登录并生成新二维码；**取消** 取消登录并清除本地会话。二维码过期、用户拒绝、网络失败和服务端验证要求都会显示状态。退出应用保留登录，重启后自动恢复书架；token 过期时刷新一次并保存轮换后的凭据。

已内置 Noto Sans SC 中文字体，桌面和 Kindle 无需额外安装字体即可显示中文书名；仍可注册补充字体：

```sh
cargo run -- --font /path/to/chinese-font.ttf
```

桌面会话默认保存于 `$XDG_DATA_HOME/rereader/session.json`，未设置时使用 `~/.local/share/rereader/session.json`；可用 `REREADER_DATA_DIR` 指定专用目录。Kindle 启动器默认使用 `/var/local/rereader/`，支持通过同一环境变量覆盖；目录需要可写并支持 Unix 权限。目录权限为 `0700`、凭据文件为 `0600`，凭据不会写入应用日志。

封面在后台并发加载并转换为灰度缩略图，缓存在会话目录下的 `covers/`，缓存键按账号与封面 URL 隔离；加载失败时显示书名占位，不影响翻页和搜索。仅从允许的腾讯图片域名加载，不向封面主机发送账号 token。

协议层位于 `src/weread/`，后台任务与 Slint 状态绑定位于 `src/app.rs`。网络请求支持取消，界面按操作代次丢弃旧结果。开发预览截图不启动网络或读取账号。

## 开发与预览

需要 Rust 1.92 或更新版本。

```sh
cargo run
# 无需图形环境，使用同一份 Slint UI 生成 600×800 灰度截图
cargo run -- --output preview.pgm
cargo test
```

默认 `desktop` feature 使用 winit 窗口后端和软件渲染。Kindle 构建关闭桌面依赖。
界面在 `ui/app.slint`，包括登录二维码、书架和分页按钮。基础窗口回调在
`src/main.rs`；`build.rs` 编译并嵌入 Slint 资源；`src/software.rs` 用于无窗口截图与 UI 渲染测试。
Kindle 的显示、触摸和事件循环直接交给上游后端，不需要额外屏幕刷新脚本。

## 构建 Kindle 安装包

需要可执行的 `clang`（用于 TLS 依赖的 ARM C/汇编代码）；Rust 链接继续使用 `rust-lld`。可通过目标专用的 `CC_armv7_unknown_linux_musleabihf` 覆盖 C 编译器。

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

默认嵌入 Noto Sans SC 中文字体，中文书名和界面在桌面、截图与 Kindle 上使用同一字体。
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
本项目代码使用 MIT；协议实现参考 `weread-omni`，其许可见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。Slint、后端和字体分别遵循各自许可，分发时保留相应声明。

## 当前验证

本地测试覆盖模拟扫码登录、token 刷新轮换、书架解析、封面缓存与地址校验、中文字体字形、搜索、会话文件权限、取消请求及网格/二维码渲染；Clippy 检查通过。另有默认忽略的在线二维码测试，已实际通过匿名票据和二维码创建请求；尚未使用账号扫码确认或验证真实书架。

已通过 ARMv7 musl 静态 release 构建。Kindle 真机运行尚未验证；软浮点目标目前仅提供构建配置。

参考：[后端 API 与设备说明](https://github.com/sverrejb/slint-kindle-backend)、
[Slint Rust 文档](https://docs.slint.dev/latest/docs/rust/slint/)。
