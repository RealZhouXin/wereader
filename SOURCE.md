# 对应源码与分发

Copyright (c) 2026 rereader contributors.

rereader 采用 GPL-3.0-only，不提供任何担保。完整许可见 `LICENSE`；第三方许可见
`THIRD_PARTY_NOTICES.md`、字体许可文件及源码包的 `vendor/` 目录。

## 获取对应源码

本安装包的发布者应在同一下载页面提供 `rereader-<target>-source.tar.gz`，
与 `rereader-<target>.tar.gz` 对应。源码包由同一次打包生成，包含实际用于构建的
项目源码、资源、锁定依赖源码及构建和安装脚本。若收到的二进制没有对应源码入口，
请向该二进制的发布者索取；最新开发分支不能替代对应版本的源码。

## 重建和安装

解压源码包并进入 `rereader-source/`。需要 Rust 1.92 或更新版本、Cargo、clang、
tar 和目标 Rust 标准库；这些通用工具不包含在源码包中。
`BUILD_INFO` 记录原构建的目标及 Rust/Cargo 版本，`Cargo.lock` 固定依赖版本。
`.cargo/config.toml` 已配置使用包内的 `vendor/`，不需要从 crates.io 下载依赖。

```sh
rustup target add armv7-unknown-linux-musleabihf
cargo build --offline --locked --release --no-default-features --features kindle \
  --target armv7-unknown-linux-musleabihf
# 生成可安装包和新的对应源码包
sh scripts/package.sh armv7-unknown-linux-musleabihf
```

软浮点设备改用 `arm-unknown-linux-musleabi`。桌面可用 `cargo run --offline --locked`。
Kindle 安装步骤见 `README.md`：设备需要支持执行自定义程序，将安装目录复制到 USB
根目录，再使用 KUAL 或 `sh /mnt/us/rereader/start.sh` 启动。
本应用没有为修改版二进制设置额外签名或授权密钥。

## 再分发

发布修改版时，保留许可和版权声明，注明修改及日期，并重新生成与二进制对应的源码包。
下载页面必须同时提供二进制和对应源码，并明确二者关系；源码不得另行收费。
包内说明不能替代实际提供源码。若连同消费设备转让，还需按 GPLv3 第 6 条适用条件
提供安装修改版所需的信息。完整义务以 `LICENSE` 为准。
