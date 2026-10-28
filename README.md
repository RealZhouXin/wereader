# rereader · Rust + Slint

一个支持微信读书扫码登录与书架展示的 Kindle 原生应用，UI 使用 Slint，设备后端使用
[sverrejb/slint-kindle-backend](https://github.com/sverrejb/slint-kindle-backend)。
与同目录的微信读书安装包相互独立。

微信读书接入采用墨水屏客户端路线：

- [微信读书墨水屏 API 协议](docs/weread-api.md)：统一说明设备配置、登录鉴权、接口参数、正文解包、进度签名、笔记定位及实测/复测范围，参考 weread-omni 与 weread-kindle 的固定源码快照。
- [wxBook 墨水屏实现研究](docs/wxbook-interface-research.md)：正文、进度及缓存实现参考；其 HTTP 层采用独立的 Web 会话协议。
- [SQLite 与本地文件存储设计](docs/storage-design.md)：账号、书架、进度、笔记和同步表结构，以及正文/图片文件索引；附建表 SQL、迁移与 Rust 存储接口。

## 扫码登录与书架

运行 `cargo run`，点击 **微信扫码登录**，使用微信扫描二维码并在手机上确认。登录成功后加载电子书书架，以每页 4 列 × 3 行展示封面和书名；底部箭头翻页，**刷新** 更新书架，**退出登录** 删除本地登录凭据，保留该账号的书架、进度和笔记供下次登录恢复。顶部支持按书名或作者搜索，读完的书显示标记。微信读书已有分组直接显示为书架中的封面卡片，以组内前四本书的封面组成 2×2 拼图，组内书籍不在书架重复展开；点击卡片进入分组，左上角返回书架并恢复页码。书架搜索覆盖所有书籍，分组内搜索只查询当前分组；分组随书架缓存，断网仍可查看。当前只展示电子书列表，不提供打开正文或有声书入口。

**更换二维码** 会取消旧登录并生成新二维码；**取消** 取消登录并清除本地会话。二维码过期、用户拒绝、网络失败和服务端验证要求都会显示状态。退出应用保留登录，重启后先显示本地书架与封面，再尝试网络更新；token 过期时刷新一次并保存轮换后的凭据。

点击底部人像进入“我的”，可查看账号的累计阅读时长和本周阅读时间。登录、恢复会话及刷新书架成功后更新这两项数据；时长按小时和分钟显示，不足一分钟单独提示，本周数据采用微信读书当前周统计。加载时显示“加载中…”，缺失字段或请求失败显示“暂不可用”，可点击刷新重试。账号数据仅保存在当前页面内存中，退出登录时清空。

已内置 Noto Sans SC 中文字体，桌面和 Kindle 无需额外安装字体即可显示中文书名；仍可注册补充字体：

```sh
cargo run -- --font /path/to/chinese-font.ttf
```

桌面数据库默认保存于 `$XDG_DATA_HOME/rereader/rereader.sqlite3`，未设置时使用 `~/.local/share/rereader/rereader.sqlite3`；可用 `REREADER_DATA_DIR` 指定专用目录。Kindle 启动器默认使用 `/var/local/rereader/`，支持通过同一环境变量覆盖；目录需要可写并支持 Unix 权限。目录权限为 `0700`、数据库和缓存文件为 `0600`，凭据不会写入应用日志。首次启动会事务导入有效旧 `session.json`，成功后移除旧文件；已有数据库时不再次导入，避免登出后旧 token 恢复。损坏的旧会话不导入，需要重新登录。

封面在后台并发加载并转换为灰度缩略图，保存在数据目录下的 `images/<account_hash>/cover/`，SQLite 记录文件索引、版本、校验值与路径，缓存键按账号与封面 URL 隔离；加载失败时显示书名占位，不影响翻页和搜索。仅从允许的腾讯图片域名加载，不向封面主机发送账号 token。

存储层位于 `src/storage/`，SQLite 和文件 I/O 由独立线程串行处理，支持账号隔离、章节、正文/插图文件索引、双端进度快照、笔记/书签/划线、阅读设置和持久化同步队列。正文阅读及笔记 UI 尚未接入；进度和笔记的远端写协议验证后，调用方才可启用对应同步任务。

界面参考 [电子书书架原型](https://www.figma.com/design/wAVCmw7NPd6vFnUzllfJS0?node-id=3-2)、[weread-kindle](https://github.com/Zz2217967571/weread-kindle) 的书架网格及官方墨水屏分组截图，使用纯白色背景、细线搜索框、4×3 封面网格和底部胶囊翻页控件，按窗口尺寸缩放。顶部显示实际书籍总数和同步状态；底部罗盘用于同步书架，书本返回书架，人像打开账号页，可退出登录或退出应用。原型的导入、选择和发现业务尚未接入，顶部提供现有刷新与账号操作。封面继续使用账号书架的真实图片，缺失或加载失败时显示书名和作者。

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

需要可执行的 `clang`、`llvm-ar`、`make`、`curl`、`tar`、`sha256sum`。macOS 可通过 `brew install llvm` 安装 `llvm-ar`，并将 `$(brew --prefix llvm)/bin` 加入 `PATH`。SQLite 与 TLS 的 ARM C 代码使用 `scripts/clang-musl.sh`，静态归档使用 `llvm-ar`，避免 macOS 的 `ar`/`ranlib` 无法处理 ARM ELF 对象而生成空归档；首次构建需安装固定 musl 1.2.5 头文件，Rust 提供静态 libc 和 `rust-lld`。可通过目标专用的 `CC_armv7_unknown_linux_musleabihf`、`AR_armv7_unknown_linux_musleabihf` 覆盖 C 编译器和归档工具，或用 `REREADER_MUSL_SYSROOT` 指定具有 `include/` 的自备 musl sysroot。

```sh
rustup target add armv7-unknown-linux-musleabihf
# package.sh 会自动准备头文件；直接 cargo build 前先手动执行此步
sh scripts/setup-musl-headers.sh armv7-unknown-linux-musleabihf
sh scripts/package.sh
```

若此前在 macOS 上构建出现 `sqlite3_*` 或 `ring_core_*` 未定义符号，更新归档工具配置后先清除旧的 C 依赖产物，再重新构建（`ring` 不会自动跟踪归档工具环境变量的变化）：

```sh
cargo clean -p libsqlite3-sys -p ring --release --target armv7-unknown-linux-musleabihf
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
sh scripts/setup-musl-headers.sh arm-unknown-linux-musleabi
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
Copyright (c) 2026 rereader contributors.

本项目采用 **GNU GPL 第 3 版（仅此版本，`GPL-3.0-only`）**，完整条款见 [LICENSE](LICENSE)。
你可以依照该许可使用、修改和再分发本程序；本程序不提供任何担保，详见许可中的免责声明。
Slint 选择 GPLv3 许可，包含 Slint 的桌面及 Kindle 完整程序均按 GPLv3 分发。
协议实现参考 `weread-omni`，保留其 MIT 声明；后端、字体及其他第三方组件保留各自许可，
详见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

`scripts/package.sh` 当前默认仅生成本地调试用的 `dist/rereader-<target>.tar.gz` 安装包，
跳过源码快照和 `cargo vendor`。需要同时生成对应源码包时运行
`sh scripts/package.sh --with-source`（也可同时指定目标）；额外生成
`dist/rereader-<target>-source.tar.gz`。源码包包含本项目源码、资源、
构建及安装脚本、`Cargo.lock` 和通过 `cargo vendor --locked` 收集的依赖源码与许可。
打包需要能获取锁定版本的全部依赖。发布时必须同时上传这两个文件，在二进制下载处提供
清晰且无需额外付费的对应源码下载入口，并保持源码可用；不能只提供最新分支链接。
具体重建与分发说明见 [SOURCE.md](SOURCE.md)。

## 当前验证

本地测试覆盖模拟扫码登录、token 刷新轮换、书架解析、封面缓存与地址校验、中文字体字形、搜索、取消请求及网格/二维码渲染；存储测试另覆盖旧会话迁移、权限、版本升级、账号代次、事务回滚、进度/注释并发修改、同步 ACK、结果不确定恢复、文件校验及 LRU。Clippy 检查通过。另有默认忽略的在线二维码测试，已实际通过匿名票据和二维码创建请求。2026-10-27 使用本地已有会话验证了真实书架、书籍信息、目录和 EPUB 单章 ZIP 下载，随后完成该章密码解包、CRC、身份和 XHTML 解析验证，详见解密实测记录。阅读 UI 尚未接入；本次没有重新扫码登录。

已通过 ARMv7 musl 静态 release 构建。Kindle 真机运行尚未验证；软浮点目标目前仅提供构建配置。

参考：[后端 API 与设备说明](https://github.com/sverrejb/slint-kindle-backend)、
[Slint Rust 文档](https://docs.slint.dev/latest/docs/rust/slint/)。
