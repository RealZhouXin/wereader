# rereader：基于 weread-omni 的墨水屏协议研究

研究日期：2026-10-26。路线已选定为墨水屏客户端协议。

补充材料：[wxBook 墨水屏实现研究](wxbook-interface-research.md)。该项目提供正文及进度代码参考，但其 HTTP 层采用独立的 Web 会话，不能直接补齐本文客户端协议中的下载和上报实现。

参考项目：[teng-lin/weread-omni](https://github.com/teng-lin/weread-omni)。本次通过远端 HEAD 查询确认研究快照仍为 `88bd2e095d7d7ee423eaadf8f40653e72c5be6d4`；仓库 `package.json` 标记版本为 `0.1.2`，运行环境为 Node.js ≥22.13.0。下文链接固定到该提交，源码快照不能直接等同于 npm 发布产物。

## 1. 研究结论

该项目适合用作 Rust 客户端的登录、令牌管理、请求传输、书架、目录和笔记协议参考。它没有实现普通电子书的章节正文下载、资源解析或阅读进度写回。

| 阅读器能力 | 仓库中的证据 | 对 rereader 的意义 |
| --- | --- | --- |
| 扫码登录 | `auth/qrlogin.ts` 有票据、二维码、轮询、凭据交换调用 | 可按协议移植 |
| 令牌刷新 | `auth/token.ts` 有刷新、并发合并、凭据轮换 | 可按生命周期移植 |
| 书架、书籍、目录 | `api/resources/shelf.ts`、`book.ts` 有实际 HTTP 调用 | 可先完成 UI 和数据层 |
| 云端进度读取 | `book.progress()` 调用 `/book/getProgress` | 可读取远端位置，偏移语义待联调 |
| 划线/想法 | `notes.ts`、`review.ts` 有读写调用 | 可后续接入 |
| 普通书籍正文 | `book.ts` 没有正文方法；`chapterdownload` 仅见于端点文档 | 需要另行研究客户端下载链路 |
| 阅读位置/时长写回 | `/book/read`、`batchUploadProgress` 仅见于端点文档 | 需要另行确认请求构造与签名 |

这里的“实际 HTTP 调用”指源码中有实现。本次没有登录账号、访问私人书架、读取正文或上传进度，也没有运行上游测试。

### 容易误读的三处

1. **导出 `ChapterContent` 不代表实现正文读取。** [类型文件][types] 明确将它定义为供扩展客户端使用的数据类型；[缓存封装][cache] 只有在外部客户端提供 `book.chapterContent` 时才包装该方法。默认 [book 模块][book] 仅有 `info`、`detail`、`chapters`、`progress`。
2. **公众号文章正文不等于电子书正文。** 项目的文章读取帮助函数处理公众号来源，不能替代普通书籍章节下载。
3. **端点目录不等于 SDK 能力清单。** [端点文档][catalog] 将大量未实现调用标为 `—`；其中正好包括电子书下载和阅读上报。文档记录了客户端类名和字段名，没有给出完整请求构造算法。

## 2. 从入口到请求的调用链

```text
CLI / TypeScript 调用方
  → AccountManager / einkProvider
  → WeReadClient（暴露各资源方法）
  → MobileApiClient（绑定当前会话、创建资源模块）
  → resources/book.ts、shelf.ts 等
  → MobileClient.call / callRaw
  → TokenManager.get
  → fetch → https://i.weread.qq.com
```

[WeReadClient][client] 将方法委托给墨水屏客户端；[MobileApiClient][mobile-client] 延迟创建传输层，并维护登录会话代次。多请求操作可以固定在同一个会话上，避免中途重新登录后把前后两次请求混到不同账号。

对 Rust 的对应关系建议如下。这是 rereader 的设计安排，尚未实现：

| Rust 模块 | 参考实现 | 职责 |
| --- | --- | --- |
| `weread/profile.rs` | `profile.ts`、`device-ua.ts` | 设备信息、版本头、签名策略 |
| `weread/auth.rs` | `auth/qrlogin.ts` | 二维码登录状态机 |
| `weread/session.rs` | `auth/token.ts`、`mobile-client.ts` | 凭据、刷新合并、账号代次 |
| `weread/transport.rs` | `api/mobile.ts` | JSON/字节请求、业务错误、重试边界 |
| `weread/books.rs` | `resources/book.ts`、`shelf.ts` | 书架、书籍详情、目录、进度读取 |
| `weread/content.rs` | 当前仓库没有完整对应实现 | 章节下载、资源处理和正文解析 |
| `weread/progress.rs` | 当前只有端点记录 | 位置/时长上报和离线队列 |

Kindle 产物建议保留原生 Rust 数据层；Node SDK 适合作为开发机联调参照。将整个 Node 运行环境放入当前 Kindle 安装包需要额外适配，不能直接使用现有 ARM musl 打包方案。

## 3. 设备参数与鉴权契约

从 [device-ua.ts][device] 和 [profile.ts][profile] 核对到：

| 字段 | 参考值/规则 |
| --- | --- |
| API 基地址 | `https://i.weread.qq.com` |
| 业务身份请求头 | `vid`、`accessToken` |
| `User-Agent` | `WeRead/2.1.2 WRBrand/Onyx wr_eink Dalvik/2.1.0 (Linux; U; Android 11; BOOX Build/onyx)` |
| `appver`、`basever` | `2.1.2.10245900` |
| `baseapi` | `30` |
| `osver` | `11` |
| `channelId` | `900` |
| 登录请求 `deviceName` | `BOOX` |
| 登录请求 `deviceType` | 数字 `3` |
| 新设备 ID | `eink334691225` 前缀 + 19 位十进制随机部分 |
| 新安装 ID | `eink31` 前缀 + 26 位随机数字 |

这些是参考项目采用的客户端配置，不是服务端正式公开的 Kindle 参数。移植时应集中管理整套配置；不能只修改 UA，同时把版本、设备类型和签名留在不一致的组合中。

业务请求使用 `vid`、`accessToken` 请求头。登录成功后将数字形式的 `vid` 规范为字符串，并持久化刷新所需的设备身份和凭据。

## 4. 扫码登录：可以完整移植的部分

调用顺序见 [qrlogin.ts][qr]：

1. `GET https://i.weread.qq.com/wxticket?nonceStr=weread`，取得 `signature` 和 `timeStamp`。
2. `GET https://open.weixin.qq.com/connect/sdk/qrconnect`，携带票据、应用标识和 scope，取得 `uuid`。
3. 将 `https://open.weixin.qq.com/connect/confirm?uuid={uuid}` 绘制成二维码。
4. 轮询 `GET https://long.open.weixin.qq.com/connect/l/qrconnect?f=json&uuid={uuid}`，后续携带上一次状态 `last`。
5. 确认后取得 `wx_code`，发送到 `POST https://i.weread.qq.com/login` 换取客户端凭据。

参考实现的微信应用标识为 `wxab9b71ad2b90ff34`，scope 为 `snsapi_userinfo,snsapi_timeline,snsapi_friend`。这是被研究客户端的参数，不代表可独立申请或任意替换的第三方 OAuth 配置。

| 轮询业务码 | 客户端处理 |
| --- | --- |
| `404` | 已扫描，等待确认 |
| `405` | 已确认，读取 `wx_code` |
| `402` | 二维码过期 |
| `403` | 用户拒绝 |
| `408` | 等待超时后继续轮询 |

默认单次轮询超时 65 秒，整体期限 5 分钟，等待超时后的轮询间隔 1 秒。这些是 SDK 的客户端策略，不是已确认的服务端有效期。

`/login` 的扫码交换请求包含：`appFirstInstall=1`、`code`、`deviceId`、`deviceName`、`installId`、`isAutoLogout=0`、`isFromQrcode=1`、`random`、`signature`、`timestamp`、`trackId=""`、`deviceType=3`。

源码中的签名规则为：将毫秒时间戳、设备 ID、十进制随机数按顺序无分隔拼接，计算 SHA-256，输出小写十六进制。扫码交换的随机数范围为 0～999。

成功响应必须有非空 `accessToken`、`refreshToken` 和合法 `vid`。不要只根据 HTTP 200 判断登录成功。

## 5. 刷新与会话隔离

[token.ts][token] 仍调用 `POST /login`，请求字段变为 `refreshToken`、设备信息、`inBackground=0`、`kickType=1`、`refCgi=""`、`trackId=""`、时间戳、随机数和签名。

当前墨水屏 profile 的刷新签名仍为 `SHA256(timestamp + deviceId + random)`，没有将 `refreshToken` 拼入签名；刷新随机数为 1～1000。这个规则只核对了登录/刷新，不能推广到阅读上报。

需要保留的行为：

- 并发请求共用一次刷新，单个调用取消只结束自己的等待。
- 刷新响应的 `vid` 必须与当前账号一致。
- 服务端可能轮换 refreshToken，更新后的凭据需要持久化。
- 新登录后，旧会话的刷新回调不能覆盖新会话凭据；参考实现使用代次和串行写入处理。
- 当前 SDK 没有固定“每 N 分钟刷新”的策略；优先使用缓存 token，在鉴权失败后按请求语义决定刷新及重放。

[凭据保存实现][credentials] 使用权限受限的目录/文件和临时文件重命名。rereader 应保存到自己的账号目录，避免与开发机 SDK 的账号存储互相覆盖。

## 6. 传输层：移植时不能丢掉的行为

[mobile.ts][transport] 分开提供 `call()` 和 `callRaw()`：前者解析 JSON，后者返回字节。二者都识别业务错误；字节响应若实际是 token 过期的 JSON，不能缓存成电子书。

| 情况 | 参考实现行为 |
| --- | --- |
| HTTP 401 或业务码 `-2012` | 按会话失效处理；可重放请求最多重试一次 |
| GET/HEAD | 默认可重放 |
| 只读 POST，如目录 | 资源方法显式设置 `idempotent: true` |
| 非幂等写入遇鉴权失败 | 可刷新供后续调用使用，但不自动重放当前写入；结果可能不确定 |
| 业务码 `-2041` | 归为人机验证要求 |
| 业务错误 JSON | 同时兼容 `errCode`/`errcode` 和对应消息字段 |
| 重定向 | `redirect: "error"`，不自动跟随 |
| 原始字节下载 | 可设置 `maxResponseBytes`；调用方仍需校验真实格式 |

尤其需要注意：`callRaw()` 只是下载传输能力，没有实现解包、解码或解密。它只接受当前 API 基地址内的路径；如果下载响应返回 `res.weread.qq.com` 的 URL，需要单独确认下载主机、认证和跳转策略，不能盲目转发 API 身份头。

## 7. 已实现的最小数据接口

本节基地址统一为 `https://i.weread.qq.com`。

| 方法/路径 | 输入 | 原始响应与注意点 |
| --- | --- | --- |
| `GET /shelf/sync` | SDK 无业务参数 | `books` 必须是数组；`albums` 和 `archive` 可缺省 |
| `GET /book/info` | `bookId` | 多数字段可缺省，未知字段仍在原始 JSON 中 |
| `POST /book/chapterInfos` | `bookIds:[id]`、`synckeys:[0]`、`updateTimes:[0]`、`maxfreeIdx:[0]` | 章节位于 `data[0].updated`，SDK 再转为 `chapters` |
| `GET /book/getProgress` | `bookId` | 读取 `book.chapterUid`、`chapterOffset`、`progress` 等 |
| `GET /store/search` | `keyword`、`scope`、`maxIdx`、可选 `count` | 默认电子书 `scope=10`；返回 `books[].bookInfo` |

证据：[book.ts][book]、[shelf.ts][shelf]、[search.ts][search]、[types.ts][types]。

Rust 的响应模型建议仅将必需标识作为严格字段，其余使用可选字段，并保留调研所需的未知元数据。特别是当前 `BookInfo` 类型没有声明 `bookVersion`，不能据此认定服务端不会返回它。

`chapterUid`、`chapterIdx` 和本地页码应分开；`synckey` 是同步游标，不能统一当作分页偏移。进度的偏移单位、百分比和时长口径仍需通过墨水屏客户端请求和账号读回验证。

## 8. 阅读闭环还缺什么

从[端点目录][catalog]能确定调查位置，但不能确定完整协议：

| 目标 | 已知入口/客户端类 | 下一步必须获取的证据 |
| --- | --- | --- |
| 单章下载 | `GET /book/chapterdownload`；`BaseBookDownloadService` / `BookDownloadService` | 调用方如何组成 `chapters`，`bookVersion` 从哪里来，`pf/pfkey` 如何取得，响应返回 URL 还是内容 |
| 整书下载 | `GET /book/download`；同上 | 适用格式、返回结构、权限与资源链路 |
| 单次阅读上报 | `POST /book/read`；`BaseReportService` / `ReportService` | 实际必填字段组合、签名串、偏移单位、时长累计/增量含义 |
| 离线批量上报 | `POST /book/batchUploadProgress`；同上 | `books` 元素结构、记录时间、签名和重复提交语义 |

`chapterdownload` 的目录记录还包含 `bookType`、`offline`、`preload`、`preview`、`quote`、`release`、`stopAutoPayWhenBNE`、`zoneId`。字段名不足以确定默认值和业务语义，不应试填成固定常量。

阅读上报目录中同时存在 `progress`、`currentProgress`、`chapterProgress`，也有多种阅读时长字段。应分别确认各字段的含义和单位，并追踪客户端 `signature` 的构造规则。

该仓库不包含完成以上调用所需的完整客户端实现。下一份关键材料是与端点目录版本相符的墨水屏客户端 APK，或本人账号正常阅读产生的请求/响应样本：

1. 从书架打开一本有权阅读的书，记录元数据、目录和下载请求的先后关系。
2. 跟踪 `BookDownloadService` 的调用方和返回值消费方，确定资源获取、文件格式及解析入口。
3. 正常阅读、翻页、退出后观察 `ReportService`，核对请求字段与手机端恢复位置。
4. 分别验证免费、会员可读和个人导入书，确认权限及格式差异，不把一种格式的成功推广到所有书籍。

这是后续调查清单，尚未完成 APK 分析或真实请求采样。

## 9. 上游测试能证明什么

[二维码测试][qr-test] 使用注入的假响应验证流程、设备字段和签名；[传输测试][transport-test] 覆盖请求失败、鉴权和重试边界。这些是有价值的移植参考。

[只读在线测试][live-test] 默认跳过，需显式启用；其内容主要是书架、笔记、搜索和点评。[回放测试][cassette-test] 的公开样本为搜索元数据，不是章节正文响应。即便这些测试通过，也不足以证明正文下载或进度写回可用。

## 10. rereader 的实施顺序

1. Rust 实现设备配置、扫码登录和凭据保存，桌面 UI 展示二维码及状态。
2. 实现传输和刷新，再接书架、书籍详情、目录、云端位置读取。
3. 根据 APK/正常阅读样本补齐单章下载，只先验证一种正文格式。
4. 建立原文偏移到本地分页的映射，并验证进度写回和跨设备恢复。
5. 接入离线缓存、图片和笔记，再做 Kindle 休眠与恢复联调。

第二阶段可以形成可操作的登录/书架/目录应用，但“可阅读”的验收应以第三、第四阶段的正文与位置闭环为准。

移植或复制实现时保留项目的 [MIT 许可及作者声明][license]。这份文档仅完成源码研究，没有改动应用运行逻辑。

[client]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/api/client.ts
[mobile-client]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/api/mobile-client.ts
[profile]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/profile.ts
[device]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/device-ua.ts
[qr]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/auth/qrlogin.ts
[token]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/auth/token.ts
[credentials]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/auth/credentials.ts
[transport]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/api/mobile.ts
[book]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/api/resources/book.ts
[shelf]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/api/resources/shelf.ts
[search]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/api/resources/search.ts
[types]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/api/types.ts
[cache]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/src/library/cached-client.ts
[catalog]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/docs/endpoints.md
[qr-test]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/test/integration/qrlogin.test.ts
[transport-test]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/test/integration/mobile-transport.test.ts
[live-test]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/test/live/read-only.test.ts
[cassette-test]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/test/integration/live-api-cassette.test.ts
[license]: https://github.com/teng-lin/weread-omni/blob/88bd2e095d7d7ee423eaadf8f40653e72c5be6d4/LICENSE
